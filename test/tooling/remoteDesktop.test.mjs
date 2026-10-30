// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeEngine } from './fixtures.mjs';
import { build } from 'esbuild';

const result = await build({
	stdin: { contents: `export { RemoteDesktopService } from './src/workbench/services/remoteDesktop/remoteDesktopServiceImpl.js'; export { isEngineCommand, isEngineReply } from './src/platform/engine/common/engine.js'; export { probeWindowDecoders } from './src/workbench/services/remoteDesktop/windowDecoders.js';`, loader: 'ts', resolveDir: process.cwd() },
	bundle: true, write: false, format: 'esm', platform: 'node', tsconfig: 'tsconfig.json',
	// The window's channel client and decoder need a renderer; the tests give the service their own.
	plugins: [{
		name: 'stubs', setup(builder) {
			builder.onResolve({ filter: /(dataChannelClient|webCodecsDecoder|extensions)\.js$/ }, args => ({ path: args.path, namespace: 'stub' }));
			builder.onLoad({ filter: /.*/, namespace: 'stub' }, () => ({
				contents: 'export const DataChannelClient = { open() { throw new Error("no channel in tests"); } }; export const createWebCodecsDecoder = async () => undefined; export const webCodecsSupport = async () => undefined; export const registerSingleton = () => {}; export const InstantiationType = {};',
				loader: 'js',
			}));
		},
	}],
});

const { RemoteDesktopService, isEngineCommand, isEngineReply, probeWindowDecoders } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

const codecs = { encode_baseline: false, encode_high: false, decode_baseline: true, decode_high: false, max_width: 3840, max_height: 2160, hardware: true, device: 'test', chroma444: false };

test('the codecs this computer handles are read from the engine and frozen', async () => {
	const calls = [];
	const service = new RemoteDesktopService(fakeEngine({
		engineCommand: async command => {
			calls.push(command.type);
			return command.type === 'videoCodecs' ? { type: 'videoCodecs', codecs } : { type: 'done' };
		}
	}, true));
	const read = await service.codecs();
	assert.deepEqual(read, codecs);
	assert(Object.isFrozen(read));
	assert.deepEqual(calls, ['videoCodecs']);
	const broken = new RemoteDesktopService(fakeEngine({ engineCommand: async () => ({ type: 'done' }) }, true));
	await assert.rejects(broken.codecs(), /Invalid video codecs reply/);
});

test('a session names its connection and nothing of a picture', () => {
	const session = { id: '1', generation: '7', role: 'Controller', peer_id: 'peer', phase: 'Active' };
	assert(isEngineReply({ type: 'sessions', sessions: [session] }));
	for (const extra of [{ quality: null }, { monitor_width: 1920 }, { monitor_height: 1080 }]) {
		assert(!isEngineReply({ type: 'sessions', sessions: [{ ...session, ...extra }] }), `a session with ${Object.keys(extra)[0]} is refused`);
	}
	for (const command of [
		{ type: 'updateSessionQuality', sessionId: '1', generation: '7', role: 'Controller', quality: { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 } },
		{ type: 'selectDisplays', sessionId: '1', generation: '7', ids: ['1'] },
	]) {
		assert(!isEngineCommand(command), `${command.type} is no engine command`);
	}
});

test('codec preferences reject malformed or extra fields', () => {
	for (const codec of ['auto', 'h264', 'h265', 'vp8', 'vp9', 'av1']) {
		assert(isEngineCommand({ type: 'setDesktopCodecDefault', codec }));
	}
	for (const command of [{ type: 'setDesktopCodecDefault', codec: 'bogus' }, { type: 'setDesktopCodecDefault' }, { type: 'setDesktopCodecDefault', codec: 'vp9', sessionId: '1' }]) {
		assert(!isEngineCommand(command));
	}
});

test('a screen opens only where it is shared: the picture first, then the input when controlling', async () => {
	const engine = fakeEngine({
		engineCommand: async command => command.type === 'controllerSessions'
			? { type: 'sessions', sessions: [{ id: '4', generation: '2', role: 'Controller', peer_id: 'peer', phase: 'Active' }] }
			: { type: 'done' }
	}, true);
	const statuses = { desktopControl: 'turnedOff', desktopView: 'available' };
	const devices = { availability: async () => Object.entries(statuses).map(([capability, status]) => ({ capability, status })) };
	const service = new RemoteDesktopService(engine, devices);
	const opened = [];
	service.openChannel = async request => {
		opened.push(request.kind);
		if (request.kind === 'input' && statuses.refuseInput) {
			throw new Error('Unauthorized: control is not allowed');
		}
		return {
			onCursor: () => ({ dispose() { } }),
			input() { }, close() {
				opened.push(`closed ${request.kind}`);
			}
		};
	};
	assert.deepEqual(await service.open('peer', true), { failure: 'turnedOff' });
	assert.deepEqual(opened, [], 'a refused screen opened a channel');
	const viewing = await service.open('peer', false);
	assert(viewing.session && !viewing.session.control);
	assert.deepEqual(opened, ['video'], 'viewing opened input');
	viewing.session.dispose();
	opened.length = 0;
	statuses.desktopControl = 'available';
	const controlling = await service.open('peer', true);
	assert(controlling.session?.control);
	assert.deepEqual(opened, ['video', 'input']);
	controlling.session.dispose();
	opened.length = 0;
	statuses.refuseInput = true;
	assert.deepEqual(await service.open('peer', true), { failure: 'turnedOff' });
	assert.deepEqual(opened, ['video', 'input', 'closed video'], 'a refused input left the picture open');
});

test('the window says which codecs it decodes, hardware ones marked with their matrix, each once, and whether it shows stills', async () => {
	// A hardware decoder that shows neither matrix right answers nothing for `prefer-hardware`, as VP9's does here.
	// H.264 shows BT.709 in hardware; H.265 only BT.601, as Chromium's VA-API path converts.
	const answers = { 'avc1.640028': { 'prefer-hardware': 'bt709', 'prefer-software': 'bt709' }, 'hev1.1.6.L153.B0': { 'prefer-hardware': 'bt601', 'prefer-software': 'bt709' }, 'vp09.00.40.08': { 'prefer-software': 'bt709' }, 'av01.0.08M.08': { 'prefer-software': 'bt709' }, 'av01.1.08M.08.0.000': { 'no-preference': 'bt709' } };
	const decoders = await probeWindowDecoders(async config => answers[config.codec]?.[config.hardwareAcceleration]);
	assert.deepEqual(decoders, [{ codec: 'h264', hardware: true, chroma444: false, bt601: false }, { codec: 'h265', hardware: true, chroma444: false, bt601: true }, { codec: 'vp9', hardware: false, chroma444: false, bt601: false }, { codec: 'av1', hardware: false, chroma444: true, bt601: false }]);
	assert(isEngineCommand({ type: 'setWindowDecoders', decoders }));
	assert(isEngineCommand({ type: 'setWindowDecoders', decoders: [] }));
	for (const bad of [
		[{ codec: 'h264', hardware: true, chroma444: false, bt601: false }, { codec: 'h264', hardware: false, chroma444: false, bt601: false }],
		[{ codec: 'mjpeg', hardware: true, chroma444: false, bt601: false }],
		[{ codec: 'h264', hardware: true, bt601: false }],
		[{ codec: 'h264', hardware: true, chroma444: false }],
		[{ codec: 'h264', hardware: true, chroma444: 1, bt601: false }],
		[{ codec: 'h264', hardware: true, chroma444: false, bt601: 1 }],
		[{ codec: 'h264', hardware: true, chroma444: false, bt601: false, extra: 1 }],
	]) {
		assert(!isEngineCommand({ type: 'setWindowDecoders', decoders: bad }), JSON.stringify(bad));
	}
});

test('a screen offers only what the window decodes, and leaves out a codec it could not show', async () => {
	const calls = [];
	const engine = fakeEngine({
		engineCommand: async command => {
			calls.push(command);
			return command.type === 'controllerSessions'
				? { type: 'sessions', sessions: [{ id: '4', generation: '2', role: 'Controller', peer_id: 'peer', phase: 'Active' }] }
				: { type: 'done' };
		},
	}, true);
	const devices = { availability: async () => [{ capability: 'desktopView', status: 'available' }] };
	const service = new RemoteDesktopService(engine, devices);
	let probes = 0;
	service.decoderSupport = async config => {
		probes++;
		return config.codec === 'avc1.640028' || (config.codec === 'hev1.1.6.L153.B0' && config.hardwareAcceleration === 'prefer-software') ? 'bt709' : undefined;
	};
	let consume;
	service.openChannel = async (request, consumer) => {
		if (request.kind === 'video') {
			consume = consumer;
		}
		return { onCursor: () => ({ dispose() { } }), input() { }, requestKeyframe() { }, mediaStatus: async () => undefined, close() { } };
	};
	const registered = () => calls.filter(command => command.type === 'setWindowDecoders').map(command => command.decoders);
	const first = await service.open('peer', false);
	assert.deepEqual(registered(), [[{ codec: 'h264', hardware: true, chroma444: false, bt601: false }, { codec: 'h265', hardware: false, chroma444: false, bt601: false }]]);
	// The decoder this test's window has can show nothing: the H.264 keyframe ends it.
	const ends = [];
	first.session.onEnd(end => ends.push(end));
	const keyframe = new Uint8Array([0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0xac]);
	await consume({ type: 'data', sequence: 1, kind: 'video', data: keyframe, metadata: JSON.stringify({ codec: 'h264', keyframe: true, timestampUs: '1', displayId: '7', revision: '3', hostUs: 0, queuedUs: 0, role: 'changed' }) });
	assert.deepEqual(ends, ['cantShow']);
	const asked = probes;
	const second = await service.open('peer', false);
	assert.deepEqual(registered().at(-1), [{ codec: 'h265', hardware: false, chroma444: false, bt601: false }], 'the codec it could not show was offered again');
	assert.equal(probes, asked, 'the window was asked again');
	second.session.dispose();
});

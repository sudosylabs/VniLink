import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeEngine } from './fixtures.mjs';
import { build } from 'esbuild';

const result = await build({
	stdin: { contents: `export { RemoteDesktopService } from './src/workbench/services/remoteDesktop/remoteDesktopServiceImpl.js'; export { DesktopReadbackError, isValidDesktopQuality } from './src/workbench/services/remoteDesktop/remoteDesktopService.js'; export { isEngineCommand, isEngineReply } from './src/platform/engine/common/engine.js'; export { probeWindowDecoders } from './src/workbench/services/remoteDesktop/windowDecoders.js';`, loader: 'ts', resolveDir: process.cwd() },
	bundle: true, write: false, format: 'esm', platform: 'node', tsconfig: 'tsconfig.json',
	// The window's channel client and decoder need a renderer; the tests give the service their own.
	plugins: [{
		name: 'stubs', setup(builder) {
			builder.onResolve({ filter: /(dataChannelClient|webCodecsDecoder|extensions)\.js$/ }, args => ({ path: args.path, namespace: 'stub' }));
			builder.onLoad({ filter: /.*/, namespace: 'stub' }, () => ({
				contents: 'export const DataChannelClient = { open() { throw new Error("no channel in tests"); } }; export const createWebCodecsDecoder = async () => undefined; export const registerSingleton = () => {}; export const InstantiationType = {};',
				loader: 'js',
			}));
		},
	}],
});

const { RemoteDesktopService, DesktopReadbackError, isValidDesktopQuality, isEngineCommand, isEngineReply, probeWindowDecoders } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

const originalQuality = { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 };

function fixture() {
	const controller = { id: '1', generation: '7', role: 'Controller', phase: 'Active', peer_id: 'peer', quality: { ...originalQuality } };
	const host = { ...controller, role: 'Host', generation: '3', quality: { ...originalQuality } };
	const calls = [];
	let failReadback = false;
	let written = false;
	const api = {
		engineCommand: async command => {
			calls.push(command);
			if (written && failReadback) {
				throw new Error('Lost engine after commit');
			}
			if (command.type === 'controllerSessions') {
				return { type: 'sessions', sessions: [controller, { ...controller, id: '2', quality: null }] };
			}
			if (command.type === 'hostedSessions') {
				return { type: 'sessions', sessions: [host] };
			}
			if (command.type === 'snapshot') {
				return { type: 'snapshot', snapshot: { devices: [{ endpoint_id: 'peer', pet_name: 'Work', hostname: null }] } };
			}
			if (command.type === 'videoCodecs') {
				return { type: 'videoCodecs', codecs: { encode_baseline: false, encode_high: false, decode_baseline: true, decode_high: false, max_width: 3840, max_height: 2160, hardware: true, device: 'test', chroma444: false } };
			}
			if (command.type === 'sessionTopology' || command.type === 'topology') {
				return { type: 'displays', displays: [] };
			}
			if (command.type === 'updateSessionQuality') {
				(command.role === 'Controller' ? controller : host).quality = { ...command.quality };
				written = true;
				return { type: 'done' };
			}
			throw new Error('Unexpected command');
		}
	};
	return {
		controller, host, calls, service: new RemoteDesktopService(fakeEngine(api, true)), failReadback: () => {
			failReadback = true;
		}
	};
}

test('desktop sessions preserve role and generation and omit non-desktop sessions', async () => {
	const f = fixture();
	const settings = await f.service.load();
	assert.equal(settings.sessions.length, 2);
	assert.equal(settings.sessions[0].name, 'Work');
	await f.service.displays(settings.sessions[0]);
	assert(f.calls.some(c => c.type === 'sessionTopology' && c.sessionId === '1' && c.generation === '7'));
	await f.service.displays(settings.sessions[1]);
	assert(f.calls.some(c => c.type === 'topology'));
	const quality = { ...originalQuality, max_frame_rate: 60 };
	const state = await f.service.apply(settings.sessions[1], quality);
	assert.deepEqual(f.controller.quality, originalQuality);
	assert.deepEqual(state.sessions[1].quality, quality);
	assert(f.calls.some(c => c.type === 'updateSessionQuality' && c.role === 'Host' && c.generation === '3'));
});

test('reconnected sessions and invalid limits never receive a write', async () => {
	const f = fixture();
	const target = (await f.service.load()).sessions[0];
	f.controller.generation = '8';
	await assert.rejects(f.service.apply(target, originalQuality));
	await assert.rejects(f.service.apply(target, { ...originalQuality, max_frame_rate: 61 }));
	assert(!f.calls.some(c => c.type === 'updateSessionQuality'));
	assert(!isValidDesktopQuality({ ...originalQuality, max_bitrate_kbps: 0 }));
	assert(!isValidDesktopQuality({ ...originalQuality, max_width: 7681 }));
});

test('quality drafts are captured and accepted writes with missing readback are explicit', async () => {
	const f = fixture();
	const target = (await f.service.load()).sessions[0];
	const draft = { ...originalQuality, max_frame_rate: 24 };
	const saving = f.service.apply(target, draft);
	draft.max_frame_rate = 60;
	await saving;
	assert.equal(f.controller.quality.max_frame_rate, 24);
	const other = fixture();
	const owner = (await other.service.load()).sessions[0];
	other.failReadback();
	await assert.rejects(other.service.apply(owner, originalQuality), DesktopReadbackError);
});

test('quality IPC requires a generation and rejects malformed negotiated quality', () => {
	const command = { type: 'updateSessionQuality', sessionId: '1', generation: '7', role: 'Controller', quality: originalQuality };
	assert(isEngineCommand(command));
	const { generation, ...missing } = command;
	assert.equal(generation, '7');
	assert(!isEngineCommand(missing));
	assert(!isEngineReply({ type: 'sessions', sessions: [{ id: '1', generation: '7', role: 'Controller', peer_id: 'peer', phase: 'Active', quality: { bad: true } }] }));
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
			? { type: 'sessions', sessions: [{ id: '4', generation: '2', role: 'Controller', peer_id: 'peer', phase: 'Active', quality: null }] }
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

test('the window says which codecs it decodes, hardware ones marked, each once', async () => {
	const answers = { 'avc1.640028': ['prefer-hardware', 'no-preference'], 'hev1.1.6.L153.B0': ['no-preference'] };
	const decoders = await probeWindowDecoders(async config => (answers[config.codec] ?? []).includes(config.hardwareAcceleration));
	assert.deepEqual(decoders, [{ codec: 'h264', hardware: true }, { codec: 'h265', hardware: false }]);
	assert(isEngineCommand({ type: 'setWindowDecoders', decoders }));
	assert(isEngineCommand({ type: 'setWindowDecoders', decoders: [] }));
	for (const bad of [
		[{ codec: 'h264', hardware: true }, { codec: 'h264', hardware: false }],
		[{ codec: 'mjpeg', hardware: true }],
		[{ codec: 'h264' }],
		[{ codec: 'h264', hardware: true, extra: 1 }],
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
				? { type: 'sessions', sessions: [{ id: '4', generation: '2', role: 'Controller', peer_id: 'peer', phase: 'Active', quality: null }] }
				: { type: 'done' };
		},
	}, true);
	const devices = { availability: async () => [{ capability: 'desktopView', status: 'available' }] };
	const service = new RemoteDesktopService(engine, devices);
	let probes = 0;
	service.decoderSupport = async config => {
		probes++;
		return config.codec === 'avc1.640028' || (config.codec === 'hev1.1.6.L153.B0' && config.hardwareAcceleration === 'no-preference');
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
	assert.deepEqual(registered(), [[{ codec: 'h264', hardware: true }, { codec: 'h265', hardware: false }]]);
	// The decoder this test's window has can show nothing: the H.264 keyframe ends it.
	const ends = [];
	first.session.onEnd(end => ends.push(end));
	const keyframe = new Uint8Array([0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0xac]);
	await consume({ type: 'data', sequence: 1, kind: 'video', data: keyframe, metadata: JSON.stringify({ codec: 'h264', keyframe: true, timestampUs: '1', displayId: '7', revision: '3', hostUs: 0, queuedUs: 0 }) });
	assert.deepEqual(ends, ['cantShow']);
	const asked = probes;
	const second = await service.open('peer', false);
	assert.deepEqual(registered().at(-1), [{ codec: 'h265', hardware: false }], 'the codec it could not show was offered again');
	assert.equal(probes, asked, 'the window was asked again');
	second.session.dispose();
});

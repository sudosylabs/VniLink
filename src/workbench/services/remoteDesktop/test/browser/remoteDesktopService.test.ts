// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { DisposableStore } from '../../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { IDataChannelService } from '../../../../../platform/engine/common/dataChannelService.js';
import type { EngineCommand, EngineReply, RemoteStatus, VideoCodecCaps, WindowDecoder } from '../../../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../../../platform/engine/common/engineService.js';
import { TestDataChannelService } from '../../../../../platform/engine/test/common/testDataChannelService.js';
import { TestEngineService } from '../../../../../platform/engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { IVideoDecoderService } from '../../../camera/videoDecoderService.js';
import { TestVideoDecoderService } from '../../../camera/test/common/testVideoDecoderService.js';
import { IDeviceService } from '../../../devices/deviceService.js';
import { INativeRuntimeService } from '../../../nativeRuntime/nativeRuntimeService.js';
import type { DesktopEnd, IDesktopSession } from '../../remoteDesktopService.js';
import { RemoteDesktopService } from '../../remoteDesktopServiceImpl.js';

const codecs: VideoCodecCaps = { encode_baseline: false, encode_high: false, decode_baseline: true, decode_high: false, max_width: 3840, max_height: 2160, hardware: true, device: 'test', chroma444: false };

/** An H.264 High profile keyframe's first bytes, as the other computer sends one. */
const keyframe = new Uint8Array([0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0xac]);

/** The engine of a computer connected to `peer` as session 4 of generation 2, which accepts every other command. */
function connected(command: EngineCommand): EngineReply {
	return command.type === 'controllerSessions'
		? { type: 'sessions', sessions: [{ id: '4', generation: '2', role: 'Controller', peer_id: 'peer', phase: 'Active' }] }
		: { type: 'done' };
}

suite('RemoteDesktopService', () => {
	const disposables = new DisposableStore();
	let engine: TestEngineService;
	let dataChannels: TestDataChannelService;
	let decoders: TestVideoDecoderService;
	let statuses: { desktopControl: RemoteStatus; desktopView: RemoteStatus };
	setup(() => {
		engine = disposables.add(new TestEngineService(connected));
		dataChannels = new TestDataChannelService();
		decoders = new TestVideoDecoderService();
		statuses = { desktopControl: 'available', desktopView: 'available' };
	});
	teardown(() => {
		// The service releases its `onEnd` listener when a session ends, not when it is disposed,
		// so a session still open ends as one whose picture was lost before it is disposed.
		for (const channel of dataChannels.openedFor('video')) {
			if (!channel.closed) {
				channel.onError(new Error('Data channel disconnected'));
			}
		}
		disposables.clear();
	});

	/** The service, as the workbench builds it. */
	function createService(): RemoteDesktopService {
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IEngineService, engine);
		instantiationService.stub(IDeviceService, {
			availability: (id: string) => {
				assert.strictEqual(id, 'peer');
				return Promise.resolve([{ capability: 'desktopControl', status: statuses.desktopControl }, { capability: 'desktopView', status: statuses.desktopView }]);
			},
		});
		instantiationService.stub(INativeRuntimeService, {});
		instantiationService.stub(IDataChannelService, dataChannels);
		instantiationService.stub(IVideoDecoderService, decoders);
		return instantiationService.createInstance(RemoteDesktopService);
	}

	/** The screen of `peer`, opened, for the test to dispose. */
	async function openScreen(service: RemoteDesktopService, control: boolean): Promise<IDesktopSession> {
		const result = await service.open('peer', control);
		assert.ok('session' in result, 'the screen did not open');
		return disposables.add(result.session);
	}

	/** The decoder lists the service registered with the engine, oldest first. */
	function registered(): WindowDecoder[][] {
		return engine.commands.flatMap(command => command.type === 'setWindowDecoders' ? [command.decoders] : []);
	}

	/** A window that decodes H.264 in hardware and H.265 only in software, in BT.709. */
	function decodeH264AndSoftwareH265(): void {
		decoders.supportOf = config => config.codec === 'avc1.640028' || (config.codec === 'hev1.1.6.L153.B0' && config.hardwareAcceleration === 'prefer-software') ? 'bt709' : undefined;
	}
	test('the codecs this computer handles are read from the engine and frozen', async () => {
		engine.handler = command => command.type === 'videoCodecs' ? { type: 'videoCodecs', codecs } : { type: 'done' };
		const read = await createService().codecs();
		assert.deepStrictEqual(read, codecs);
		assert.ok(Object.isFrozen(read));
		assert.deepStrictEqual(engine.commands.map(command => command.type), ['videoCodecs']);
	});
	test('an engine that answers the codecs with anything else gives an invalid reply', async () => {
		engine.handler = () => ({ type: 'done' });
		await assert.rejects(createService().codecs(), /Invalid video codecs reply/);
	});
	test('a screen whose control is turned off refuses control and opens no channel', async () => {
		statuses.desktopControl = 'turnedOff';
		assert.deepStrictEqual(await createService().open('peer', true), { failure: 'turnedOff' });
		assert.deepStrictEqual(dataChannels.requests, []);
	});
	test('viewing a screen opens its picture and no input', async () => {
		statuses.desktopControl = 'turnedOff';
		const session = await openScreen(createService(), false);
		assert.strictEqual(session.control, false);
		assert.deepStrictEqual(dataChannels.requests, [{ kind: 'video', sessionId: '4', generation: '2' }]);
	});
	test('controlling a screen opens its picture, then its input', async () => {
		const session = await openScreen(createService(), true);
		assert.strictEqual(session.control, true);
		assert.deepStrictEqual(dataChannels.requests.map(request => request.kind), ['video', 'input']);
	});
	test('a refused input closes the picture it followed and reports control turned off', async () => {
		dataChannels.refuse = request => request.kind === 'input' ? new Error('Unauthorized: control is not allowed') : undefined;
		assert.deepStrictEqual(await createService().open('peer', true), { failure: 'turnedOff' });
		assert.deepStrictEqual(dataChannels.requests.map(request => request.kind), ['video', 'input']);
		assert.deepStrictEqual(dataChannels.closes.map(request => request.kind), ['video'], 'a refused input left the picture open');
	});
	test('opening a screen tells the engine which codecs the window decodes', async () => {
		decodeH264AndSoftwareH265();
		await openScreen(createService(), false);
		assert.deepStrictEqual(registered(), [[{ codec: 'h264', hardware: true, chroma444: false, bt601: false }, { codec: 'h265', hardware: false, chroma444: false, bt601: false }]]);
	});
	test('a keyframe the window has no decoder for ends the session as one it can\'t show', async () => {
		decodeH264AndSoftwareH265();
		decoders.decodes = false;
		const session = await openScreen(createService(), false);
		const ends: DesktopEnd[] = [];
		disposables.add(session.onEnd(end => ends.push(end)));
		await dataChannels.opened[0].consume({ type: 'data', sequence: 1, kind: 'video', data: keyframe, metadata: JSON.stringify({ codec: 'h264', keyframe: true, timestampUs: '1', displayId: '7', revision: '3', hostUs: 0, queuedUs: 0, role: 'changed' }) });
		assert.deepStrictEqual(ends, ['cantShow']);
	});

	/** A service that opened one screen whose H.264 keyframe this window could not show. */
	async function afterUnshownH264(): Promise<RemoteDesktopService> {
		decodeH264AndSoftwareH265();
		decoders.decodes = false;
		const service = createService();
		await openScreen(service, false);
		await dataChannels.opened[0].consume({ type: 'data', sequence: 1, kind: 'video', data: keyframe, metadata: JSON.stringify({ codec: 'h264', keyframe: true, timestampUs: '1', displayId: '7', revision: '3', hostUs: 0, queuedUs: 0, role: 'changed' }) });
		return service;
	}
	test('a codec the window could not show is left out of the next offer', async () => {
		const service = await afterUnshownH264();
		await openScreen(service, false);
		assert.deepStrictEqual(registered().at(-1), [{ codec: 'h265', hardware: false, chroma444: false, bt601: false }]);
	});
	test('the window is asked which codecs it decodes only once', async () => {
		const service = await afterUnshownH264();
		const asked = decoders.probed.length;
		await openScreen(service, false);
		assert.strictEqual(decoders.probed.length, asked);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

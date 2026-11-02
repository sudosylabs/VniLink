// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../../base/common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import type { DataPacket } from '../../../../../platform/engine/common/dataChannel.js';
import { IDataChannelService } from '../../../../../platform/engine/common/dataChannelService.js';
import type { RemoteStatus } from '../../../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../../../platform/engine/common/engineService.js';
import { TestDataChannelService, type TestDataChannel } from '../../../../../platform/engine/test/common/testDataChannelService.js';
import { TestEngineService } from '../../../../../platform/engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { IDeviceService } from '../../../devices/deviceService.js';
import { INativeRuntimeService } from '../../../nativeRuntime/nativeRuntimeService.js';
import type { CameraEnd, CameraFailure, ICameraStream } from '../../cameraService.js';
import { cameraOpenFailure, cameraRefusal, CameraService } from '../../cameraServiceImpl.js';
import { EngineCameraStream } from '../../engineCameraStream.js';
import { IVideoDecoderService } from '../../videoDecoderService.js';
import { TestVideoDecoderService } from '../common/testVideoDecoderService.js';

/** An access unit as the host sends one: SPS, PPS, then the picture. */
const keyframe = Uint8Array.from([0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0xda, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 1, 0x65, 0x88]);

const delta = Uint8Array.from([0, 0, 0, 1, 0x41, 0x9a]);

/** A picture of the host's camera, as its channel delivers one. */
function picture(data: Uint8Array, isKeyframe: boolean, sequence = 1): DataPacket {
	return { type: 'data', sequence, kind: 'camera', data, metadata: JSON.stringify({ codec: 'h264', keyframe: isKeyframe, timestampUs: '33333', width: 640, height: 480 }) };
}

/** The host's last packet, saying why the picture stopped. */
function exited(reason: string, sequence = 2): DataPacket {
	return { type: 'data', sequence, kind: 'exited', metadata: JSON.stringify({ reason }) };
}

/** A real one-pixel picture, as a decoder outputs one. */
function videoFrame(): VideoFrame {
	return new VideoFrame(new Uint8Array(4), { format: 'RGBA', codedWidth: 1, codedHeight: 1, timestamp: 0 });
}

/** A closed `VideoFrame` has released its picture and no longer names a format. */
function isClosed(frame: VideoFrame): boolean {
	return frame.format === null;
}

suite('CameraService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let dataChannels: TestDataChannelService;
	let decoders: TestVideoDecoderService;
	setup(() => {
		dataChannels = new TestDataChannelService();
		decoders = new TestVideoDecoderService();
	});

	/** The service, as the workbench builds it, on a computer connected to `peer` as session 4 of generation 2. */
	function createService(): CameraService {
		const engine = disposables.add(new TestEngineService(command => {
			assert.strictEqual(command.type, 'controllerSessions');
			return { type: 'sessions', sessions: [{ id: '4', generation: '2', role: 'Controller', peer_id: 'peer', phase: 'Active' }] };
		}));
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IEngineService, engine);
		instantiationService.stub(IDeviceService, {});
		instantiationService.stub(INativeRuntimeService, {});
		instantiationService.stub(IDataChannelService, dataChannels);
		instantiationService.stub(IVideoDecoderService, decoders);
		return instantiationService.createInstance(CameraService);
	}

	/** The camera of `peer`, open, with the channel it arrives on and the ends it reported. */
	async function openCamera(): Promise<{ stream: ICameraStream; channel: TestDataChannel; ends: CameraEnd[] }> {
		const result = await createService().open('peer', 'front');
		assert.ok('stream' in result, 'the camera did not open');
		const stream = disposables.add(result.stream);
		const ends: CameraEnd[] = [];
		disposables.add(stream.onEnd(end => ends.push(end)));
		const [channel] = dataChannels.opened;
		return { stream, channel, ends };
	}
	test('opening asks the host for the camera on the active connection', async () => {
		await openCamera();
		assert.deepStrictEqual(dataChannels.requests, [{ kind: 'camera', sessionId: '4', generation: '2', media: { type: 'camera', deviceId: 'front', width: 1280, height: 720, fps: 30 } }]);
	});
	test('a predicted picture before the first keyframe opens no decoder', async () => {
		const { channel } = await openCamera();
		await channel.consume(picture(delta, false));
		assert.deepStrictEqual(decoders.decoders, []);
	});
	test('the first keyframe opens a decoder for its codec and is decoded as a key picture', async () => {
		const { channel } = await openCamera();
		await channel.consume(picture(delta, false));
		await channel.consume(picture(keyframe, true, 2));
		assert.deepStrictEqual(decoders.decoders.map(decoder => decoder.codec), ['avc1.42C01F']);
		assert.deepStrictEqual(decoders.decoders[0].chunks.map(chunk => [chunk.key, chunk.timestamp]), [[true, 33333]]);
	});
	test('a slow decoder holds back acknowledgement until a picture leaves its queue', async () => {
		const { channel } = await openCamera();
		await channel.consume(picture(keyframe, true, 1));
		await channel.consume(picture(delta, false, 2));
		let acknowledged = false;
		const third = Promise.resolve(channel.consume(picture(delta, false, 3))).then(() => {
			acknowledged = true;
		});
		await timeout(0);
		assert.strictEqual(acknowledged, false, 'three pictures waiting must hold the channel');
		decoders.decoders[0].dequeue();
		await third;
		assert.strictEqual(acknowledged, true);
	});
	test('the listener owns each decoded picture', async () => {
		const { stream, channel } = await openCamera();
		const frames: VideoFrame[] = [];
		disposables.add(stream.onFrame(frame => frames.push(frame)));
		await channel.consume(picture(keyframe, true));
		const frame = videoFrame();
		decoders.decoders[0].output(frame);
		assert.deepStrictEqual(frames, [frame]);
		assert.strictEqual(isClosed(frame), false);
		frame.close();
	});
	test('disposing the stream closes its decoder', async () => {
		const { stream, channel } = await openCamera();
		await channel.consume(picture(keyframe, true));
		stream.dispose();
		// The decoder may still be starting; it is closed once it exists.
		await timeout(0);
		assert.strictEqual(decoders.decoders[0].closed, true);
	});
	test('a picture decoded after disposal is closed, not kept', async () => {
		const { stream, channel } = await openCamera();
		const frames: VideoFrame[] = [];
		disposables.add(stream.onFrame(frame => frames.push(frame)));
		await channel.consume(picture(keyframe, true));
		stream.dispose();
		const late = videoFrame();
		decoders.decoders[0].output(late);
		assert.strictEqual(isClosed(late), true);
		assert.deepStrictEqual(frames, []);
	});
	for (const reason of ['stopped', 'withdrawn', 'lost'] as const) {
		test(`the host's last packet ending with '${reason}' ends the stream once, closing its channel and decoder`, async () => {
			const { channel, ends } = await openCamera();
			await channel.consume(picture(keyframe, true));
			await channel.consume(exited(reason));
			channel.onError(new Error('Data channel disconnected'));
			assert.deepStrictEqual(ends, [reason]);
			assert.strictEqual(dataChannels.closes.length, 1);
			assert.strictEqual(decoders.decoders[0].closed, true);
		});
	}
	const refused: readonly (readonly [string, DataPacket])[] = [
		['an end the host may not give, as it never decides this window can\'t decode', exited('cantShow', 1)],
		['a camera packet without a picture', { type: 'data', sequence: 1, kind: 'camera', metadata: '{}' }],
		['a picture with a negative timestamp', { ...picture(keyframe, true), metadata: JSON.stringify({ keyframe: true, timestampUs: '-1' }) }],
		['a packet of another channel\'s kind', { type: 'data', sequence: 1, kind: 'terminal', data: keyframe, metadata: '{}' }],
	];
	for (const [name, packet] of refused) {
		test(`${name} is refused`, async () => {
			const { channel } = await openCamera();
			await assert.rejects(Promise.resolve(channel.consume(packet)));
		});
	}
	test('a codec this window has no decoder for ends the stream as one it can\'t show', async () => {
		decoders.decodes = false;
		const { channel, ends } = await openCamera();
		await channel.consume(picture(keyframe, true));
		assert.deepStrictEqual(ends, ['cantShow']);
	});
	test('a decoder that fails ends the stream as one it can\'t show', async () => {
		const { channel, ends } = await openCamera();
		await channel.consume(picture(keyframe, true));
		decoders.decoders[0].error();
		assert.deepStrictEqual(ends, ['cantShow']);
	});
	test('a channel the host refuses as busy reports the camera in use', async () => {
		dataChannels.refuse = () => new Error('Busy: Native data channel unavailable');
		assert.deepStrictEqual(await createService().open('peer', 'front'), { failure: 'inUse' });
	});
	const refusals: readonly (readonly [RemoteStatus | undefined, CameraFailure | undefined])[] = [
		['available', undefined],
		['turnedOff', 'turnedOff'],
		['needsPermission', 'needsPermission'],
		['unavailable', 'unavailable'],
		['unsupported', 'unsupported'],
		[undefined, 'unsupported'],
	];
	for (const [status, failure] of refusals) {
		test(`a host reporting its camera as ${status ?? 'nothing'} is ${failure ? `refused as ${failure}` : 'not refused'}`, () => {
			assert.strictEqual(cameraRefusal(status), failure);
		});
	}
	const openFailures: readonly (readonly [string, unknown, CameraFailure])[] = [
		['Busy', new Error('Busy: Native data channel unavailable'), 'inUse'],
		['Unauthorized', new Error('Unauthorized: Native data channel unavailable'), 'turnedOff'],
		['NoSession', new Error('NoSession: gone'), 'disconnected'],
		['an uncategorized error', new Error('Native data channel unavailable'), 'failed'],
		['a value that is not an error', 'Busy', 'failed'],
	];
	for (const [name, error, failure] of openFailures) {
		test(`a camera that fails to open with ${name} reports ${failure}`, () => {
			assert.strictEqual(cameraOpenFailure(error), failure);
		});
	}
});

suite('EngineCameraStream', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	test('a channel that opens after the stream ended is closed', () => {
		const decoders = new TestVideoDecoderService();
		const stream = disposables.add(new EngineCameraStream((codec, output, error) => decoders.createDecoder(codec, output, error)));
		stream.lost();
		let closed = false;
		stream.attach({
			close: () => {
				closed = true;
			},
		});
		assert.strictEqual(closed, true);
	});
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const result = await build({
	stdin: {
		contents: `
			export { avcCodec } from './src/workbench/services/camera/h264.js';
			export { EngineCameraStream } from './src/workbench/services/camera/engineCameraStream.js';
			export { cameraOpenFailure, cameraRefusal } from './src/workbench/services/camera/cameraServiceImpl.js';
			export { recordingStartFailure } from './src/workbench/services/recording/sessionRecording.js';
			export { isNativeHostRequest, isPng } from './src/platform/native/common/native.js';
			export { isPictureName } from './src/platform/recording/common/recording.js';
			export { VSBuffer } from './src/base/common/buffer.js';
		`,
		loader: 'ts',
		resolveDir: process.cwd(),
	},
	bundle: true,
	write: false,
	format: 'esm',
	platform: 'node',
	tsconfig: 'tsconfig.json',
	// The service's channel opener and registration are not exercised here.
	plugins: [{
		name: 'stub-browser',
		setup(builder) {
			builder.onResolve({ filter: /(dataChannelClient|webCodecsDecoder|extensions)\.js$/ }, args => ({ path: args.path, namespace: 'stub' }));
			builder.onLoad({ filter: /.*/, namespace: 'stub' }, () => ({ contents: 'export const DataChannelClient = {}; export const createWebCodecsDecoder = undefined; export const registerSingleton = () => {}; export const InstantiationType = {};', loader: 'js' }));
		},
	}],
});

const { avcCodec, EngineCameraStream, cameraOpenFailure, recordingStartFailure, cameraRefusal, isNativeHostRequest, isPictureName, isPng, VSBuffer } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

/** An access unit as the host sends one: SPS, PPS, then the picture. */
const keyframe = Uint8Array.from([0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0xda, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 1, 0x65, 0x88]);

const delta = Uint8Array.from([0, 0, 0, 1, 0x41, 0x9a]);

const packet = (data, keyframe, sequence = 1) => ({ type: 'data', sequence, kind: 'camera', data, metadata: JSON.stringify({ codec: 'h264', keyframe, timestampUs: '33333', width: 640, height: 480 }) });

test('the codec string is read from the keyframe\'s parameter set', () => {
	assert.equal(avcCodec(keyframe), 'avc1.42C01F');
	assert.equal(avcCodec(Uint8Array.from([0, 0, 1, 0x27, 0x64, 0x00, 0x28])), 'avc1.640028');
	assert.equal(avcCodec(delta), undefined);
	assert.equal(avcCodec(Uint8Array.from([0, 0, 1, 0x67, 0x42])), undefined, 'a truncated SPS names no codec');
});

/** A decoder that records pictures and holds the queue until released. */
function fakeDecoder() {
	const decoder = { chunks: [], queue: 0, closed: false, waiters: [], codec: undefined, output: undefined, error: undefined };
	const factory = async (codec, output, error) => {
		Object.assign(decoder, { codec, output, error });
		return {
			decode: chunk => {
				decoder.chunks.push(chunk);
				decoder.queue++;
			},
			drained: limit => decoder.queue <= limit ? Promise.resolve() : new Promise(resolve => decoder.waiters.push(() => {
				if (decoder.queue <= limit) {
					resolve();
					return true;
				}
				return false;
			})),
			close: () => {
				decoder.closed = true;
			},
		};
	};
	decoder.dequeue = () => {
		decoder.queue--;
		decoder.waiters = decoder.waiters.filter(waiter => !waiter());
	};
	return { decoder, factory };
}

test('pictures decode from the first keyframe and a slow decoder holds back acknowledgement', async () => {
	const { decoder, factory } = fakeDecoder();
	const stream = new EngineCameraStream(factory);
	const frames = [];
	stream.onFrame(frame => frames.push(frame));
	// Predicted pictures before a keyframe have nothing to refer to.
	await stream.consume(packet(delta, false));
	assert.equal(decoder.codec, undefined);
	await stream.consume(packet(keyframe, true, 2));
	assert.equal(decoder.codec, 'avc1.42C01F');
	assert.deepEqual(decoder.chunks.map(chunk => [chunk.key, chunk.timestamp]), [[true, 33333]]);
	await stream.consume(packet(delta, false, 3));
	let acknowledged = false;
	const third = stream.consume(packet(delta, false, 4)).then(() => {
		acknowledged = true;
	});
	await new Promise(resolve => setTimeout(resolve, 20));
	assert.equal(acknowledged, false, 'three pictures waiting must hold the channel');
	decoder.dequeue();
	await third;
	assert.equal(acknowledged, true);
	const frame = {
		closed: false, close() {
			this.closed = true;
		}
	};
	decoder.output(frame);
	assert.deepEqual(frames, [frame], 'the listener owns each picture');
	stream.dispose();
	// The decoder may still be starting; it is closed once it exists.
	await new Promise(resolve => setImmediate(resolve));
	assert.equal(decoder.closed, true);
	const late = {
		closed: false, close() {
			this.closed = true;
		}
	};
	decoder.output(late);
	assert.equal(late.closed, true, 'a picture after disposal is closed, not kept');
});

test('the host\'s last packet says why the picture stopped, once', async () => {
	for (const reason of ['stopped', 'withdrawn', 'lost']) {
		const { decoder, factory } = fakeDecoder();
		const stream = new EngineCameraStream(factory);
		const ends = [];
		let closed = 0;
		stream.onEnd(end => ends.push(end));
		stream.attach({ close: () => closed++ });
		await stream.consume(packet(keyframe, true));
		await stream.consume({ type: 'data', sequence: 2, kind: 'exited', metadata: JSON.stringify({ reason }) });
		stream.lost();
		assert.deepEqual(ends, [reason]);
		assert.equal(closed, 1);
		assert.equal(decoder.closed, true);
	}
	const stream = new EngineCameraStream(fakeDecoder().factory);
	await assert.rejects(stream.consume({ type: 'data', sequence: 1, kind: 'exited', metadata: JSON.stringify({ reason: 'cantShow' }) }), 'the host never decides the viewer can\'t decode');
	await assert.rejects(stream.consume({ type: 'data', sequence: 1, kind: 'camera', metadata: '{}' }));
	await assert.rejects(stream.consume({ ...packet(keyframe, true), metadata: JSON.stringify({ keyframe: true, timestampUs: '-1' }) }));
	await assert.rejects(stream.consume({ type: 'data', sequence: 1, kind: 'terminal', data: keyframe, metadata: '{}' }));
});

test('a picture this computer can\'t decode ends the stream as such', async () => {
	const unsupported = new EngineCameraStream(async () => undefined);
	const ends = [];
	unsupported.onEnd(end => ends.push(end));
	await unsupported.consume(packet(keyframe, true));
	const { decoder, factory } = fakeDecoder();
	const failing = new EngineCameraStream(factory);
	failing.onEnd(end => ends.push(end));
	await failing.consume(packet(keyframe, true));
	decoder.error();
	assert.deepEqual(ends, ['cantShow', 'cantShow']);
	const detached = new EngineCameraStream(factory);
	detached.lost();
	let closed = false;
	detached.attach({
		close: () => {
			closed = true;
		}
	});
	assert.equal(closed, true, 'a channel that opens after the end is closed');
});

test('the host\'s report and refusals become the tab\'s reasons', () => {
	assert.equal(cameraRefusal('available'), undefined);
	assert.equal(cameraRefusal('turnedOff'), 'turnedOff');
	assert.equal(cameraRefusal('needsPermission'), 'needsPermission');
	assert.equal(cameraRefusal('unavailable'), 'unavailable');
	assert.equal(cameraRefusal('unsupported'), 'unsupported');
	assert.equal(cameraRefusal(undefined), 'unsupported');
	assert.equal(cameraOpenFailure(new Error('Busy: Native data channel unavailable')), 'inUse');
	assert.equal(cameraOpenFailure(new Error('Unauthorized: Native data channel unavailable')), 'turnedOff');
	assert.equal(cameraOpenFailure(new Error('NoSession: gone')), 'disconnected');
	assert.equal(cameraOpenFailure(new Error('Native data channel unavailable')), 'failed');
	assert.equal(cameraOpenFailure('Busy'), 'failed');
	assert.equal(recordingStartFailure(new Error('Unauthorized: recording is not allowed')), 'notAllowed');
	assert.equal(recordingStartFailure(new Error('NoSession: no camera is open on this connection')), 'disconnected');
	assert.equal(recordingStartFailure(new Error('Unsupported: recording storage is unavailable')), 'failed');
});

test('a window names, never places, a PNG it saves or copies', () => {
	const png = VSBuffer.wrap(Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0]));
	assert(isPng(png));
	assert(!isPng(VSBuffer.wrap(Uint8Array.from([0xff, 0xd8, 0xff, 0, 0, 0, 0, 0, 0]))), 'a JPEG is refused');
	assert(!isPng(Uint8Array.from(png.buffer)), 'bytes must arrive as a buffer');
	assert(!isPng(VSBuffer.alloc(33 * 1024 * 1024)));
	assert(isPictureName('Studio Mac camera 2026-10-19 14.32.05.png'));
	for (const name of ['../x.png', 'a/b.png', 'C:\\x.png', '.png', '.hidden.png', 'picture.jpg', 'a\0.png', `${'a'.repeat(200)}.png`]) {
		assert(!isPictureName(name), name);
	}
	assert(!isNativeHostRequest('savePicture', [3, 'Camera.png', png]), 'a window does not choose where pictures go');
	assert(isNativeHostRequest('writeClipboardPicture', [3, png]));
	assert(!isNativeHostRequest('writeClipboardPicture', [3, 'text']));
});

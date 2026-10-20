import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const result = await build({
	stdin: {
		contents: `
			export { EngineDesktopSession, desktopCodec, pointerShape } from './src/workbench/services/remoteDesktop/engineDesktopSession.js';
			export { DesktopLatency } from './src/workbench/services/remoteDesktop/desktopLatency.js';
			export { Emitter } from './src/base/common/event.js';
			export { isDesktopCursor } from './src/platform/engine/common/dataChannel.js';
			export { hidUsage } from './src/platform/engine/common/keyUsage.js';
			export { isInputCommand } from './src/platform/engine/common/dataChannel.js';
		`,
		loader: 'ts',
		resolveDir: process.cwd(),
	},
	bundle: true,
	write: false,
	format: 'esm',
	platform: 'node',
	tsconfig: 'tsconfig.json',
});

const { EngineDesktopSession, DesktopLatency, desktopCodec, pointerShape, Emitter, isDesktopCursor, hidUsage, isInputCommand } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

/** An H.264 keyframe's start: a start code, then an SPS with profile 0x64, flags 0x00, level 0x28. */
const keyframe = new Uint8Array([0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0xac]);

/** A decoder that outputs each picture as a frame stand-in with its timestamp, and records the codec asked for. */
function fixture({ control = true, decodes = true } = {}) {
	const codecs = [];
	const decoded = [];
	const closed = [];
	const failures = [];
	const createDecoder = async (codec, output, error) => {
		codecs.push(codec);
		failures.push(error);
		if (!decodes) {
			return undefined;
		}
		return {
			decode: chunk => {
				decoded.push(chunk);
				output({ timestamp: chunk.timestamp, close() { } });
			},
			drained: () => Promise.resolve(),
			close: () => closed.push(codec),
		};
	};
	const session = new EngineDesktopSession(control, { id: '4', generation: '2' }, createDecoder, () => Promise.resolve([]));
	const inputs = [];
	const keyframes = [];
	const cursors = new Emitter();
	session.attach({
		video: { onCursor: cursors.event, requestKeyframe: () => keyframes.push(true), mediaStatus: () => Promise.resolve({ reason: 'stable', receivedKbps: 1, receivedFps: 1, receiverQueueMs: 0, hostQueueMs: 0, lossPerMille: 0, rttMs: 20, encodeUs: 0, captureUs: 0 }), close() { } },
		input: control ? { input: command => inputs.push(command), close() { } } : undefined,
	});
	const pictures = [];
	const ends = [];
	session.onPicture(picture => pictures.push(picture));
	session.onEnd(end => ends.push(end));
	const packet = (fields, data = keyframe) => ({ type: 'data', sequence: 1, kind: 'video', data, metadata: JSON.stringify({ codec: 'h264', keyframe: true, timestampUs: '1000', displayId: '7', revision: '3', hostUs: 0, queuedUs: 0, ...fields }) });
	return { session, codecs, decoded, closed, inputs, pictures, ends, packet, failures, keyframes, cursors };
}

test('a picture is decoded with its own codec and keeps the display and revision it was taken on', async () => {
	const { session, codecs, pictures, packet } = fixture();
	// A predicted picture before any keyframe has nothing to start from.
	await session.consume(packet({ keyframe: false }));
	assert.deepEqual(codecs, []);
	await session.consume(packet({}));
	await session.consume(packet({ keyframe: false, timestampUs: '2000', revision: '4' }));
	assert.deepEqual(codecs, ['avc1.640028']);
	assert.deepEqual(pictures.map(picture => [picture.frame.timestamp, picture.display, picture.revision]), [[1000, '7', '3'], [2000, '7', '4']]);
});

test('another codec after a reconnect starts a new decoder at its keyframe', async () => {
	const { session, codecs, closed, packet } = fixture();
	await session.consume(packet({}));
	await session.consume(packet({ codec: 'vp9', keyframe: true, timestampUs: '5000' }, new Uint8Array([1, 2, 3])));
	assert.deepEqual(codecs, ['avc1.640028', 'vp09.00.40.08']);
	assert.deepEqual(closed, ['avc1.640028']);
	assert.equal(desktopCodec('av1', new Uint8Array()), 'av01.0.08M.08');
	assert.equal(desktopCodec('mjpeg', new Uint8Array()), undefined);
});

test('a picture this computer cannot decode ends the session once, and a malformed packet is refused', async () => {
	const { session, ends, packet } = fixture({ decodes: false });
	await session.consume(packet({}));
	await session.consume(packet({}));
	assert.deepEqual(ends, ['cantShow']);
	const other = fixture();
	await assert.rejects(other.session.consume(packet({ displayId: 7 })), /Invalid desktop packet/);
});

test('input carries the session, and a view-only session sends none', () => {
	const { session, inputs } = fixture();
	session.send({ type: 'submitLocalKey', usage: 4, down: true, text: 'a' });
	assert.deepEqual(inputs, [{ type: 'submitLocalKey', usage: 4, down: true, text: 'a', sessionId: '4', generation: '2' }]);
	session.lost();
	session.send({ type: 'releaseSessionInput' });
	assert.equal(inputs.length, 1, 'an ended session sent input');
	const viewer = fixture({ control: false });
	viewer.session.send({ type: 'submitLocalKey', usage: 4, down: true, text: 'a' });
	assert.deepEqual(viewer.inputs, []);
});

test('keys are named by where they are, as HID usages', () => {
	assert.equal(hidUsage('KeyA'), 0x04);
	assert.equal(hidUsage('KeyZ'), 0x1d);
	assert.equal(hidUsage('Digit1'), 0x1e);
	assert.equal(hidUsage('Digit0'), 0x27);
	assert.equal(hidUsage('F1'), 0x3a);
	assert.equal(hidUsage('F13'), 0x68);
	assert.equal(hidUsage('Numpad0'), 0x62);
	assert.equal(hidUsage('MetaRight'), 0xe7);
	assert.equal(hidUsage('constructor'), undefined, 'only the table\'s own keys');
	assert.equal(hidUsage('Fn'), undefined);
});

test('the input channel takes keys with their composing state and the display the pointer lands on', () => {
	const target = { sessionId: '4', generation: '2' };
	assert(isInputCommand({ type: 'submitLocalKey', ...target, usage: 0x2f, down: true, text: '', composing: true }));
	assert(!isInputCommand({ type: 'submitLocalKey', ...target, usage: 4, down: true, text: 'a' }), 'a key says whether it composes');
	assert(isInputCommand({ type: 'selectDisplays', ...target, ids: ['7'] }));
	assert(!isInputCommand({ type: 'updateSessionQuality', ...target, role: 'Controller', quality: { max_bitrate_kbps: 1, max_frame_rate: 1, max_width: 1, max_height: 1 } }), 'only input and display selection');
});

test('a decoder that loses the picture asks for a keyframe and starts again from it', async () => {
	const { session, codecs, closed, ends, packet, failures, keyframes, pictures } = fixture();
	await session.consume(packet({}));
	failures.at(-1)();
	await new Promise(resolve => setImmediate(resolve));
	assert.deepEqual(ends, [], 'a lost picture ended the session');
	assert.deepEqual(keyframes, [true]);
	assert.deepEqual(closed, ['avc1.640028']);
	// Predicted pictures wait; the next keyframe opens a new decoder.
	await session.consume(packet({ keyframe: false, timestampUs: '2000' }));
	assert.equal(codecs.length, 1);
	await session.consume(packet({ timestampUs: '3000' }));
	assert.equal(codecs.length, 2);
	assert.deepEqual(pictures.map(picture => picture.frame.timestamp), [1000, 3000]);
});

test('the other pointer keeps its shape across moves, unpremultiplied for a canvas', () => {
	const { session, cursors } = fixture();
	const pointers = [];
	session.onPointer(pointer => pointers.push(pointer));
	// Premultiplied BGRA: a half-transparent red, and a clear pixel.
	const image = { width: 2, height: 1, hotspotX: 1, hotspotY: 0, pixels: new Uint8Array([0, 0, 128, 128, 0, 0, 0, 0]) };
	assert(isDesktopCursor({ x: 5, y: 6, visible: true, image }));
	assert(!isDesktopCursor({ x: 5, y: 6, visible: true, image: { ...image, hotspotX: 2 } }), 'a hot spot outside the image');
	assert(!isDesktopCursor({ x: 5, y: 6, visible: true, image: { ...image, pixels: new Uint8Array(4) } }), 'pixels that do not fill it');
	assert.deepEqual([...pointerShape(image).rgba], [255, 0, 0, 128, 0, 0, 0, 0]);
	cursors.fire({ x: 5, y: 6, visible: true, image });
	cursors.fire({ x: 7, y: 8, visible: false });
	assert.deepEqual(pointers.map(pointer => [pointer.x, pointer.y, pointer.visible, pointer.shape?.hotspotX]), [[5, 6, true, 1], [7, 8, false, 1]]);
	assert.equal(pointers[0].shape, pointers[1].shape, 'a move made the shape again');
	session.lost();
	cursors.fire({ x: 9, y: 9, visible: true });
	assert.equal(pointers.length, 2, 'an ended session still reports the pointer');
});

test('stage times summarise as the median and the time nineteen in twenty beat, then start afresh', () => {
	const latency = new DesktopLatency();
	for (let ms = 1; ms <= 100; ms++) {
		latency.record('decode', ms);
	}
	latency.record('host', -3);
	latency.record('host', Number.NaN);
	assert.deepEqual(latency.take(), { decode: { p50: 50, p95: 95, count: 100 } }, 'clocks out of step are left out');
	assert.deepEqual(latency.take(), {});
	for (let at = 0; at < 300; at++) {
		latency.record('draw', at < 60 ? 1000 : 1);
	}
	assert.deepEqual(latency.take(), { draw: { p50: 1, p95: 1, count: 240 } }, 'only the newest pictures count');
});

test('a picture reports where its time went, from the other computer to the screen', async () => {
	const { session, packet, pictures } = fixture();
	await session.consume({ ...packet({ hostUs: 12_000, queuedUs: 3_000 }), readAt: performance.timeOrigin + performance.now() });
	assert.equal(pictures.length, 1);
	session.presented(pictures[0]);
	const { latency } = await session.stats();
	assert.deepEqual(Object.keys(latency).sort(), ['bridge', 'decode', 'draw', 'host', 'queue']);
	assert.equal(latency.host.p50, 12);
	assert.equal(latency.queue.p50, 3);
	assert(latency.bridge.p50 >= 0 && latency.decode.p50 >= 0 && latency.draw.p50 >= 0);
	// A picture sent without its capture time measures no host stage.
	await session.consume(packet({ timestampUs: '2000', keyframe: false }));
	assert.equal((await session.stats()).latency.host, undefined);
	await assert.rejects(fixture().session.consume(packet({ hostUs: -1 })), /Invalid desktop packet/);
	await assert.rejects(fixture().session.consume(packet({ queuedUs: '5' })), /Invalid desktop packet/);
});

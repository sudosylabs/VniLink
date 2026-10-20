import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const result = await build({
	stdin: {
		contents: `
			export { EngineDesktopSession, desktopCodec } from './src/workbench/services/remoteDesktop/engineDesktopSession.js';
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

const { EngineDesktopSession, desktopCodec, hidUsage, isInputCommand } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

/** An H.264 keyframe's start: a start code, then an SPS with profile 0x64, flags 0x00, level 0x28. */
const keyframe = new Uint8Array([0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0xac]);

/** A decoder that outputs each picture as a frame stand-in with its timestamp, and records the codec asked for. */
function fixture({ control = true, decodes = true } = {}) {
	const codecs = [];
	const decoded = [];
	const closed = [];
	const createDecoder = async (codec, output) => {
		codecs.push(codec);
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
	session.attach({ video: { close() { } }, input: control ? { input: command => inputs.push(command), close() { } } : undefined });
	const pictures = [];
	const ends = [];
	session.onPicture(picture => pictures.push(picture));
	session.onEnd(end => ends.push(end));
	const packet = (fields, data = keyframe) => ({ type: 'data', sequence: 1, kind: 'video', data, metadata: JSON.stringify({ codec: 'h264', keyframe: true, timestampUs: '1000', displayId: '7', revision: '3', ...fields }) });
	return { session, codecs, decoded, closed, inputs, pictures, ends, packet };
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

import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';
import { MessageChannel } from 'node:worker_threads';
import { fileURLToPath } from 'node:url';

async function load(path) {
	const output = await build({ entryPoints: [fileURLToPath(new URL(path, import.meta.url))], bundle: true, write: false, format: 'esm', platform: 'node' });
	return await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
}

const { DataChannelHost } = await load('../../src/platform/engine/electron-utility/dataChannelHost.ts');

// The client reads the preload's sandbox globals when it loads.
globalThis.vscode = { ipcRenderer: {}, ipcMessagePort: { acquire() { } }, webFrame: {}, process: {}, context: {}, webUtils: {} };

const { DataChannelClient } = await load('../../src/platform/engine/electron-browser/dataChannelClient.ts');

const { isChannelRequest } = await load('../../src/platform/engine/common/dataChannel.ts');

const { parseScreenSnapshot } = await load('../../src/workbench/services/screenSnapshot/common/screenSnapshot.ts');

const request = { kind: 'video', requestId: 'test', sessionId: '1', generation: '2' };

function fixture(kind = 'video') {
	const messages = [];
	const listeners = new Map();
	let closed = 0;
	const port = { on: (name, fn) => listeners.set(name, fn), start() { }, close() { }, postMessage: packet => messages.push(packet) };
	const packets = [];
	const inputs = [];
	const native = { read: () => packets.shift() ?? null, close: () => closed++, input: command => inputs.push(JSON.parse(command)), write() { }, resize() { }, fileOperation: async () => '{}' };
	const host = new DataChannelHost(port, { ...request, kind }, () => { });
	return {
		host, messages, packets, inputs, native, receive: data => listeners.get('message')({ data }), get closed() {
			return closed;
		}
	};
}

function jpeg(width, height) {
	return new Uint8Array([
		0xff, 0xd8, 0xff, 0xc0, 0x00, 0x0b, 0x08,
		(height >> 8) & 0xff, height & 0xff,
		(width >> 8) & 0xff, width & 0xff,
		0x01, 0x01, 0x11, 0x00, 0xff, 0xd9,
	]);
}

test('a screenshot packet rejects malformed images and accepts one bounded jpeg', () => {
	const image = jpeg(8, 8);
	const ready = { outcome: 'ready', width: 8, height: 8, capturedUnixMs: '1700000000000', displayId: 'display-7', mime: 'image/jpeg', endpointId: 'a'.repeat(64), sessionId: '1', generation: '2' };
	const parsed = parseScreenSnapshot(JSON.stringify(ready), image);
	assert.equal(parsed.status, 'ready');
	assert.equal(parsed.width, 8);
	assert.equal(parsed.capturedUnixMs, 1700000000000);
	assert.equal(parsed.endpointId, 'a'.repeat(64));
	assert.equal(parsed.generation, '2');
	assert.equal(parseScreenSnapshot(JSON.stringify({ ...ready, extra: true }), image), undefined);
	assert.equal(parseScreenSnapshot(JSON.stringify({ ...ready, width: 641 }), image), undefined);
	assert.equal(parseScreenSnapshot(JSON.stringify({ ...ready, mime: 'image/png' }), image), undefined);
	assert.equal(parseScreenSnapshot(JSON.stringify({ ...ready, capturedUnixMs: 1700000000000 }), image), undefined);
	assert.equal(parseScreenSnapshot(JSON.stringify(ready), new Uint8Array([0xff, 0xd8, 0x00, 0xff, 0xd9])), undefined);
	assert.equal(parseScreenSnapshot(JSON.stringify(ready), jpeg(4, 4)), undefined);
	assert.equal(parseScreenSnapshot(JSON.stringify(ready), jpeg(800, 8)), undefined);
	assert.deepEqual(parseScreenSnapshot(JSON.stringify({ outcome: 'needsPermission' }), undefined), { status: 'needsPermission' });
	assert.deepEqual(parseScreenSnapshot(JSON.stringify({ outcome: 'needsSourceSelection' }), undefined), { status: 'needsSourceSelection' });
	assert.deepEqual(parseScreenSnapshot(JSON.stringify({ outcome: 'cancelled' }), undefined), { status: 'cancelled' });
	assert.deepEqual(parseScreenSnapshot(JSON.stringify({ outcome: 'replaced' }), undefined), { status: 'replaced' });
	assert.equal(parseScreenSnapshot(JSON.stringify({ outcome: 'needsPermission', width: 1 }), undefined), undefined);
	assert.equal(parseScreenSnapshot(JSON.stringify({ outcome: 'asleep' }), undefined), undefined);
	assert.equal(parseScreenSnapshot(JSON.stringify({ outcome: 'allowed' }), undefined), undefined);
});

test('data requests reject loss of identity, invalid terminal sizes and additional authority', () => {
	assert.equal(isChannelRequest(request), true);
	assert.equal(isChannelRequest({ ...request, kind: ['video'] }), false);
	assert.equal(isChannelRequest({ ...request, sessionId: 1 }), false);
	assert.equal(isChannelRequest({ ...request, generation: '18446744073709551616' }), false);
	assert.equal(isChannelRequest({ ...request, extra: true }), false);
	assert.equal(isChannelRequest({ ...request, kind: 'terminal', rows: 24, columns: 80 }), true);
	assert.equal(isChannelRequest({ ...request, kind: 'terminal', rows: 0, columns: 80 }), false);
	assert.equal(isChannelRequest({ ...request, kind: 'snapshot' }), false);
	assert.equal(isChannelRequest({ ...request, kind: 'snapshot', media: { type: 'snapshot', prompt: false, access: false } }), true);
	assert.equal(isChannelRequest({ ...request, kind: 'snapshot', media: { type: 'snapshot', prompt: true, access: true } }), true);
	assert.equal(isChannelRequest({ ...request, kind: 'snapshot', media: { type: 'snapshot', prompt: false } }), false);
	assert.equal(isChannelRequest({ ...request, kind: 'snapshot', media: { type: 'camera', deviceId: 'x', width: 640, height: 360, fps: 5 } }), false);
});

test('binary output is ordered and a slow consumer cannot grow the IPC queue', () => {
	const f = fixture();
	try {
		const bytes = Buffer.from([0, 255, 19]);
		f.packets.push({ kind: 'video', data: bytes, metadata: '{}' }, { kind: 'video', data: Buffer.from([2]), metadata: '{}' });
		f.host.start(f.native);
		assert.equal(f.messages.filter(m => m.type === 'data').length, 1);
		assert.deepEqual(f.messages[1].data, bytes);
		f.receive({ type: 'ack', sequence: 1 });
		assert.equal(f.messages.filter(m => m.type === 'data').length, 2);
		assert.equal(f.messages[2].sequence, 2);
		f.receive({ type: 'ack', sequence: 1 });
		assert.equal(f.closed, 1, 'invalid acknowledgements terminate the channel');
	} finally {
		f.host.close();
	}
});

test('terminal exit is consumed before graceful channel closure', () => {
	const f = fixture('terminal');
	f.packets.push({ kind: 'exited', metadata: '{"code":7}' });
	f.host.start(f.native);
	assert.equal(f.closed, 0);
	f.receive({ type: 'ack', sequence: 1 });
	assert.equal(f.closed, 1);
	assert.equal(f.messages.at(-1).type, 'closed');
});

test('input cannot inject a different session or management operation', () => {
	const f = fixture('input');
	f.host.start(f.native);
	f.receive({ type: 'input', id: 1, command: { type: 'submitKey', sessionId: '1', generation: '2', usage: 4, down: true } });
	assert.equal(f.inputs.length, 1);
	f.receive({ type: 'input', id: 2, command: { type: 'submitKey', sessionId: '3', generation: '2', usage: 4, down: false } });
	assert.equal(f.inputs.length, 1);
	assert.equal(f.closed, 1);
});

test('close before native startup disposes the late handle exactly once', () => {
	const f = fixture();
	f.host.close();
	f.host.start(f.native);
	f.host.close();
	assert.equal(f.closed, 1);
});

async function until(predicate) {
	const deadline = performance.now() + 3000;
	while (!predicate()) {
		assert.ok(performance.now() < deadline, 'Channel event did not arrive');
		await new Promise(resolve => setTimeout(resolve, 1));
	}
}

test('renderer acknowledges only after consumption and coalesces positions without losing key transitions', async () => {
	const { port1, port2 } = new MessageChannel();
	const messages = [];
	port2.on('message', message => {
		messages.push(message);
		if (message.type === 'input') {
			port2.postMessage({ type: 'written', id: message.id });
		}
	});
	let release;
	const errors = [];
	const client = new DataChannelClient(port1, () => new Promise(resolve => {
		release = resolve;
	}), error => errors.push(error));
	try {
		port2.postMessage({ type: 'data', sequence: 1, kind: 'video', data: new Uint8Array([7]), metadata: '{}' });
		await until(() => release !== undefined);
		assert.equal(messages.length, 0);
		release();
		await until(() => messages.some(message => message.type === 'ack'));
		assert.equal(messages[0].type, 'ack');
		const pointer = x => ({ type: 'submitPointer', sessionId: '1', generation: '2', x, y: 0, display: '1', revision: '1' });
		client.input(pointer(1));
		client.input(pointer(2));
		client.input({ type: 'submitKey', sessionId: '1', generation: '2', usage: 4, down: true });
		client.input(pointer(3));
		await until(() => messages.filter(message => message.type === 'input').length === 3);
		const inputs = messages.filter(m => m.type === 'input').map(m => m.command);
		assert.deepEqual(inputs.map(c => c.type), ['submitPointer', 'submitKey', 'submitPointer']);
		assert.equal(inputs[0].x, 2);
		assert.equal(inputs[2].x, 3);
		assert.deepEqual(errors, []);
	} finally {
		client.close();
		port2.close();
	}
});

test('intentional close with unacknowledged input does not report a failure', async () => {
	const { port1, port2 } = new MessageChannel();
	const messages = [];
	const errors = [];
	port2.on('message', message => messages.push(message));
	const client = new DataChannelClient(port1, () => { }, error => errors.push(error));
	try {
		client.input({ type: 'submitKey', sessionId: '1', generation: '2', usage: 4, down: true });
		await until(() => messages.some(message => message.type === 'input'));
		client.close();
		await new Promise(resolve => setImmediate(resolve));
		assert.deepEqual(errors, []);
	} finally {
		client.close();
		port2.close();
	}
});

test('cancel passes through renderer and utility while progress is pending', async () => {
	const { port1, port2 } = new MessageChannel();
	let resolvePoll;
	const operations = [];
	const errors = [];
	const native = {
		close() { }, fileOperation: command => {
			const operation = JSON.parse(command);
			operations.push(operation);
			if (operation.type === 'poll') {
				return new Promise(resolve => {
					resolvePoll = resolve;
				});
			}
			return Promise.resolve('{"state":"Cancelled"}');
		}
	};
	const port = { on: (name, listener) => port2.on(name, name === 'message' ? data => listener({ data }) : listener), start() { }, close: () => port2.close(), postMessage: message => port2.postMessage(message) };
	const host = new DataChannelHost(port, { ...request, kind: 'files' }, () => { });
	host.start(native);
	// The startup handshake is handled by open(), outside the client constructor.
	await new Promise(resolve => port1.once('message', resolve));
	const client = new DataChannelClient(port1, () => { }, error => errors.push(error));
	let poll;
	try {
		poll = client.fileOperation({ type: 'poll', jobId: '1' });
		await until(() => resolvePoll !== undefined);
		await assert.rejects(client.fileOperation({ type: 'poll', jobId: '2' }), /backpressure/);
		assert.equal(await client.fileOperation({ type: 'cancel', jobId: '1' }), '{"state":"Cancelled"}');
		assert.deepEqual(operations.map(operation => operation.type), ['poll', 'cancel']);
		resolvePoll('{"state":"Cancelled"}');
		assert.equal(await poll, '{"state":"Cancelled"}');
		assert.deepEqual(errors, []);
	} finally {
		client.close();
		host.close();
		if (poll) {
			await poll.catch(() => { });
		}
	}
});

test('media channels require matching bounded source configuration and reject extra authority', () => {
	const request = { requestId: 'media', kind: 'camera', sessionId: '1', generation: '1', media: { type: 'camera', deviceId: 'camera-1', width: 1280, height: 720, fps: 30 } };
	assert(isChannelRequest(request));
	assert(!isChannelRequest({ ...request, media: undefined }));
	assert(!isChannelRequest({ ...request, kind: 'audio' }));
	assert(!isChannelRequest({ ...request, kind: 'video' }));
	assert(!isChannelRequest({ ...request, media: { ...request.media, width: 8192 } }));
	assert(!isChannelRequest({ ...request, media: { ...request.media, width: 1279 } }));
	assert(!isChannelRequest({ ...request, media: { ...request.media, fps: 61 } }));
	assert(!isChannelRequest({ ...request, media: { ...request.media, endpointId: 'other' } }));
	const audio = { ...request, kind: 'audio', media: { type: 'audio', deviceId: 'mic-1', sourceKind: 'Microphone', channels: 1, voice: true } };
	assert(isChannelRequest(audio));
	assert(!isChannelRequest({ ...audio, media: { ...audio.media, channels: 3 } }));
	assert(!isChannelRequest({ ...audio, media: { ...audio.media, sourceKind: 'output' } }));
});

test('video publication identifies its codec and refuses ambiguous packets', async () => {
	const { isEngineCommand, isEngineReply } = await load('../../src/platform/engine/common/engine.ts');
	for (const codec of ['h264', 'h265', 'vp8', 'vp9', 'av1']) {
		const command = { type: 'publishVideo', codec, sessionId: '1', keyframe: true, timestampUs: '0', data: 'AQ==' };
		assert(isEngineCommand(command));
		const { codec: ignored, ...ambiguous } = command;
		void ignored;
		assert(!isEngineCommand(ambiguous));
		assert(!isEngineCommand({ ...command, codec: 'other' }));
		assert(isEngineReply({ type: 'videoFrame', frame: { codec, keyframe: true, timestamp_us: '0', data: 'AQ==', display_id: '1', revision: '1' } }));
	}
});

test('desktop audio controls validate exact shapes and never carry decoded audio', async () => {
	const { isDesktopAudioOperation, isDesktopAudioStatus } = await load('../../src/platform/engine/common/dataChannel.ts');
	const status = { phase: 'unavailable', enabled: true, muted: false, error: { code: 'Unsupported', message: 'No output' } };
	assert(isDesktopAudioOperation({ type: 'status' }));
	assert(isDesktopAudioOperation({ type: 'muted', muted: true }));
	assert(isDesktopAudioOperation({ type: 'enabled', enabled: false }));
	assert(!isDesktopAudioOperation({ type: 'status', sessionId: 'another' }));
	assert(!isDesktopAudioOperation({ type: 'muted', muted: 1 }));
	assert(!isDesktopAudioOperation({ type: 'enabled', enabled: true, muted: true }));
	assert(isDesktopAudioStatus(status));
	assert(!isDesktopAudioStatus({ ...status, phase: 'invented' }));
	assert(!isDesktopAudioStatus({ ...status, data: new Uint8Array([1]) }));
	assert(!isDesktopAudioStatus({ ...status, error: { ...status.error, code: 'x'.repeat(65) } }));
	const f = fixture();
	try {
		f.native.desktopAudio = operation => {
			assert.deepEqual(JSON.parse(operation), { type: 'status' });
			return JSON.stringify(status);
		};
		f.host.start(f.native);
		f.receive({ type: 'desktopAudio', id: 1, operation: { type: 'status' } });
		assert.deepEqual(f.messages.at(-1), { type: 'desktopAudio', id: 1, status });
		assert.equal(f.closed, 0, 'unavailable audio must leave video running');
		f.packets.push({ kind: 'video', data: Buffer.from([7]), metadata: '{}' });
		await until(() => f.messages.some(message => message.type === 'data'));
		assert.equal(f.messages.find(message => message.type === 'data').kind, 'video');
	} finally {
		f.host.close();
	}
});

test('renderer audio requests survive stale replies and close rejects a pending control', async () => {
	const { port1, port2 } = new MessageChannel();
	const errors = [];
	const messages = [];
	port2.on('message', message => messages.push(message));
	const frames = [];
	const client = new DataChannelClient(port1, packet => frames.push(packet), error => errors.push(error));
	const status = { phase: 'muted', enabled: true, muted: true, error: null };
	try {
		const pending = client.desktopAudio({ type: 'muted', muted: true });
		await until(() => messages.some(message => message.type === 'desktopAudio'));
		const request = messages.find(message => message.type === 'desktopAudio');
		port2.postMessage({ type: 'desktopAudio', id: request.id + 100, status });
		port2.postMessage({ type: 'desktopAudio', id: request.id, status });
		assert.deepEqual(await pending, status);
		port2.postMessage({ type: 'desktopAudio', id: request.id, status });
		port2.postMessage({ type: 'data', sequence: 1, kind: 'video', data: new Uint8Array([7]), metadata: '{}' });
		await until(() => frames.length === 1);
		assert.deepEqual(errors, []);
		const closing = client.desktopAudio({ type: 'status' });
		const rejected = assert.rejects(closing, /closed/);
		client.close();
		await rejected;
		await assert.rejects(client.desktopAudio({ type: 'status' }), /unavailable/);
	} finally {
		client.close();
		port2.close();
	}
});

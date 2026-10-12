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
const { DataChannelClient } = await load('../../src/platform/engine/browser/dataChannelClient.ts');
const { isChannelRequest } = await load('../../src/platform/engine/common/dataChannel.ts');
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
test('data requests reject loss of identity, invalid terminal sizes and additional authority', () => {
	assert.equal(isChannelRequest(request), true);
	assert.equal(isChannelRequest({ ...request, kind: ['video'] }), false);
	assert.equal(isChannelRequest({ ...request, sessionId: 1 }), false);
	assert.equal(isChannelRequest({ ...request, generation: '18446744073709551616' }), false);
	assert.equal(isChannelRequest({ ...request, extra: true }), false);
	assert.equal(isChannelRequest({ ...request, kind: 'terminal', rows: 24, columns: 80 }), true);
	assert.equal(isChannelRequest({ ...request, kind: 'terminal', rows: 0, columns: 80 }), false);
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
	f.host.close(); f.host.start(f.native); f.host.close();
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
		client.input(pointer(1)); client.input(pointer(2));
		client.input({ type: 'submitKey', sessionId: '1', generation: '2', usage: 4, down: true });
		client.input(pointer(3));
		await until(() => messages.filter(message => message.type === 'input').length === 3);
		const inputs = messages.filter(m => m.type === 'input').map(m => m.command);
		assert.deepEqual(inputs.map(c => c.type), ['submitPointer', 'submitKey', 'submitPointer']);
		assert.equal(inputs[0].x, 2); assert.equal(inputs[2].x, 3);
		assert.deepEqual(errors, []);
	} finally {
		client.close(); port2.close();
	}
});

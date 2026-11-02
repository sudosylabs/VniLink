// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

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
globalThis.vnilink = { ipcRenderer: {}, ipcMessagePort: { acquire() { }, release() { } }, webFrame: {}, process: { env: {} }, context: {}, webUtils: {} };

const { DataChannelClient } = await load('../../src/platform/engine/electron-browser/dataChannelClient.ts');

const request = { kind: 'video', requestId: 'test', sessionId: '1', generation: '2' };

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

test('a packet keeps when the engine read it, and a malformed read time fails the channel', async () => {
	const { port1, port2 } = new MessageChannel();
	const consumed = [];
	const errors = [];
	const client = new DataChannelClient(port1, packet => {
		consumed.push(packet);
		return Promise.resolve();
	}, error => errors.push(error));
	try {
		port2.postMessage({ type: 'data', sequence: 1, kind: 'video', data: new Uint8Array([7]), metadata: '{}', readAt: 1234.5 });
		await until(() => consumed.length === 1);
		assert.equal(consumed[0].readAt, 1234.5);
		port2.postMessage({ type: 'data', sequence: 2, kind: 'video', data: new Uint8Array([7]), metadata: '{}', readAt: 'now' });
		await until(() => errors.length === 1);
		assert.equal(consumed.length, 1);
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

test('file operations run side by side, a refusal is an answer, and job changes arrive unasked', async () => {
	const { port1, port2 } = new MessageChannel();
	let resolveList;
	const operations = [];
	const errors = [];
	const packets = [];
	const job = { id: '7', peer: 'p', label: 'a.txt', direction: 'Upload', sender: true, state: 'Transferring', total_bytes: '100', done_bytes: '40', files_total: '1', files_done: '0', resumed_files: '0', skipped_files: '0', skipped_symlinks: '0', waiting_file: null, current_file: null, error: null, destination: null };
	const pushed = [{ kind: 'jobs', metadata: JSON.stringify({ jobs: [job] }) }];
	const native = {
		read: () => pushed.shift() ?? null,
		close() { },
		fileOperation: command => {
			const operation = JSON.parse(command);
			operations.push(operation);
			if (operation.type === 'list') {
				return new Promise(resolve => {
					resolveList = resolve;
				});
			}
			if (operation.type === 'rename') {
				return Promise.resolve('{"error":"FileExists: the name is taken"}');
			}
			return Promise.resolve(JSON.stringify({ ok: { ...job, state: 'Cancelled' } }));
		},
	};
	const port = { on: (name, listener) => port2.on(name, name === 'message' ? data => listener({ data }) : listener), start() { }, close: () => port2.close(), postMessage: message => port2.postMessage(message) };
	const host = new DataChannelHost(port, { ...request, kind: 'files' }, () => { });
	host.start(native);
	// The startup handshake is handled by open(), outside the client constructor.
	await new Promise(resolve => port1.once('message', resolve));
	const client = new DataChannelClient(port1, packet => packets.push(packet), error => errors.push(error));
	let list;
	try {
		list = client.fileOperation({ type: 'list', root: '0', path: 'Users', offset: '0' });
		await until(() => resolveList !== undefined);
		// A slow listing holds up neither a rename nor a cancel.
		assert.deepEqual(await client.fileOperation({ type: 'rename', root: '0', path: 'Users', name: 'a.txt', token: null, newName: 'b.txt' }), { error: 'FileExists: the name is taken' });
		assert.equal((await client.fileOperation({ type: 'cancel', jobId: '7' })).ok.state, 'Cancelled');
		await until(() => packets.length === 1);
		assert.equal(packets[0].kind, 'jobs');
		assert.deepEqual(JSON.parse(packets[0].metadata).jobs[0], job);
		resolveList(JSON.stringify({ ok: { entries: [], total: '0', nextOffset: null, truncated: false, skipped: '0' } }));
		assert.equal((await list).ok.total, '0');
		assert.deepEqual(operations.map(operation => operation.type), ['list', 'rename', 'cancel']);
		assert.deepEqual(errors, []);
	} finally {
		client.close();
		host.close();
		if (list) {
			await list.catch(() => { });
		}
	}
});

test('renderer audio requests survive stale replies and close rejects a pending control', async () => {
	const { port1, port2 } = new MessageChannel();
	const errors = [];
	const messages = [];
	port2.on('message', message => messages.push(message));
	const frames = [];
	const client = new DataChannelClient(port1, packet => frames.push(packet), error => errors.push(error));
	const status = { phase: 'muted', enabled: true, muted: true, playbackFailed: false, error: null };
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

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const output = await build({ entryPoints: ['src/platform/engine/electron-utility/engineHost.ts'], bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json' });

const { EngineHost } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

/** A native engine whose event queue and failures the test controls. */
function nativeEngine() {
	const native = {
		calls: [], queue: [], lagged: '0', failNextPoll: false, nextSubscription: 0,
		async execute(json) {
			const command = JSON.parse(json);
			native.calls.push(command);
			switch (command.type) {
				case 'subscribeEvents': return JSON.stringify({ type: 'eventSubscription', subscriptionId: String(++native.nextSubscription) });
				case 'pollEvents': {
					if (native.failNextPoll) {
						native.failNextPoll = false;
						throw new Error('Subscription lost');
					}
					const reply = { type: 'events', events: native.queue.splice(0), lagged: native.lagged };
					native.lagged = '0';
					return JSON.stringify(reply);
				}
				default: return JSON.stringify({ type: 'done' });
			}
		},
		openDataChannel: () => new Promise(() => { }),
	};
	return native;
}

async function openHost(native) {
	const posted = [];
	const addon = () => ({ openEngine: async () => native, openTemporaryEngine: async () => native });
	const host = new EngineHost({ postMessage: message => posted.push(message) }, addon);
	await host.open();
	assert.deepEqual(posted, [{ ready: true }]);
	return host;
}

test('events are read only while listened to, and gaps or failures are reported as a resync', async context => {
	context.mock.timers.enable({ apis: ['setTimeout'] });
	const native = nativeEngine();
	const host = await openHost(native);
	const settle = async () => {
		for (let step = 0; step < 20; step++) {
			await Promise.resolve();
		}
	};
	await settle();
	assert.equal(native.calls.length, 0, 'The host read events before anyone listened');
	const batches = [];
	const listener = host.onDidReceiveEvents(batch => batches.push(batch));
	context.mock.timers.tick(0);
	await settle();
	assert.deepEqual(batches, [{ events: [], resync: true }], 'The first batch must ask listeners to resynchronize');
	native.queue.push({ type: 'hostLocked', locked: true });
	context.mock.timers.tick(500);
	await settle();
	assert.deepEqual(batches.at(-1), { events: [{ type: 'hostLocked', locked: true }], resync: false });
	context.mock.timers.tick(500);
	await settle();
	assert.equal(batches.length, 2, 'An empty, gap-free poll must not produce a batch');
	native.lagged = '3';
	context.mock.timers.tick(500);
	await settle();
	assert.deepEqual(batches.at(-1), { events: [], resync: true });
	native.failNextPoll = true;
	context.mock.timers.tick(500);
	await settle();
	context.mock.timers.tick(500);
	await settle();
	assert.deepEqual(batches.at(-1), { events: [], resync: true });
	assert.equal(native.nextSubscription, 2, 'A failed subscription was not replaced');
	listener.dispose();
	await settle();
	const released = native.calls.filter(command => command.type === 'releaseEvents').map(command => command.subscriptionId);
	assert.deepEqual(released, ['1', '2']);
	const reads = native.calls.length;
	context.mock.timers.tick(5000);
	await settle();
	assert.equal(native.calls.length, reads, 'The host kept reading events without listeners');
});

test('commands are shape-checked, shutdown stops new work and data channels respect per-window limits', async () => {
	const native = nativeEngine();
	const host = await openHost(native);
	await assert.rejects(host.execute('snapshot'), /Invalid engine command/);
	const closed = [];
	const port = name => ({ close: () => closed.push(name), postMessage() { }, on() { }, start() { } });
	assert.equal(host.handledClientConnection({ data: { type: 'attachDataChannel', owner: 'window', request: { requestId: 'bad' } }, ports: [port('invalid')] }), true);
	assert.deepEqual(closed, ['invalid']);
	assert.equal(host.handledClientConnection({ data: 'vscode:engineHost', ports: [port('client')] }), false, 'IPC client connections must reach the server');
	await host.execute({ type: 'shutdown' });
	await assert.rejects(host.execute({ type: 'snapshot' }), /shutting down/);
	host.handledClientConnection({ data: { type: 'attachDataChannel', owner: 'window', request: { requestId: 'late', kind: 'video', sessionId: '1', generation: '1' } }, ports: [port('late')] });
	assert.deepEqual(closed, ['invalid', 'late']);
});

test('a failed open tells the main process why before exiting', async () => {
	const posted = [];
	const exits = [];
	const addon = () => ({ openEngine: () => Promise.reject(new Error('CorruptRecord')), openTemporaryEngine: () => Promise.reject(new Error('unused')) });
	const host = new EngineHost({ postMessage: message => posted.push(message) }, addon, code => exits.push(code));
	await host.open();
	assert.deepEqual(posted, [{ ready: false, error: 'CorruptRecord' }]);
	assert.deepEqual(exits, [1]);
});

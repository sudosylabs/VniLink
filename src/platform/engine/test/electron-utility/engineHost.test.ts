// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { EventEmitter } from 'events';
import { timeout } from '../../../../base/common/async.js';
import { DisposableStore } from '../../../../base/common/lifecycle.js';
import type { MessagePortMain } from '../../../../base/parts/sandbox/node/electronTypes.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../base/test/common/virtualScheduling/index.js';
import type { EngineEvent } from '../../common/engine.js';
import { ENGINE_HOST_CONNECTION, type EngineEventBatch } from '../../common/engineService.js';
import type { NativeDataChannel } from '../../electron-utility/dataChannelHost.js';
import { EngineHost } from '../../electron-utility/engineHost.js';

interface EngineCommand {
	readonly type: string;
	readonly [field: string]: unknown;
}

class TestEventWatch {
	public closed = false;

	constructor(public readonly wake: () => void) { }

	public close(): void {
		this.closed = true;
	}
}

/**
 * The native engine: a test queues events, makes the next read fail and
 * rings the wake the engine rings when an event is waiting. Every command
 * it receives is kept in `commands`.
 */
class TestNativeEngine {
	public readonly commands: EngineCommand[] = [];
	public readonly queue: EngineEvent[] = [];
	public readonly watches: TestEventWatch[] = [];
	public lagged = '0';
	public failNextPoll = false;
	public subscriptions = 0;

	/** Rings the wake as the engine does when an event is waiting. */
	public wake(): void {
		for (const watch of this.watches.filter(candidate => !candidate.closed)) {
			watch.wake();
		}
	}

	public watchEvents(wake: () => void): TestEventWatch {
		const watch = new TestEventWatch(wake);
		this.watches.push(watch);
		return watch;
	}

	public execute(json: string): Promise<string> {
		const command = JSON.parse(json) as EngineCommand;
		this.commands.push(command);
		switch (command.type) {
			case 'subscribeEvents':
				return Promise.resolve(JSON.stringify({ type: 'eventSubscription', subscriptionId: String(++this.subscriptions) }));
			case 'pollEvents': {
				if (this.failNextPoll) {
					this.failNextPoll = false;
					return Promise.reject(new Error('Subscription lost'));
				}
				const reply = { type: 'events', events: this.queue.splice(0, command.maxEvents as number), lagged: this.lagged };
				this.lagged = '0';
				return Promise.resolve(JSON.stringify(reply));
			}
			default:
				return Promise.resolve(JSON.stringify({ type: 'done' }));
		}
	}

	public openDataChannel(): Promise<NativeDataChannel> {
		return new Promise(() => { /* The channel never opens. */ });
	}

	public reads(): number {
		return this.commands.filter(command => command.type === 'pollEvents').length;
	}
}

/** A port a data channel arrives on; it records whether it was closed. */
class TestPort extends EventEmitter implements MessagePortMain {
	public closed = false;

	public close(): void {
		this.closed = true;
	}

	public postMessage(): void { /* The window end is not read here. */ }

	public start(): void { /* Messages are not queued here. */ }
}

/** Lets a read the host started finish: every promise callback runs before the next task. */
function settled(): Promise<void> {
	return timeout(0);
}

const locked = (index: number): EngineEvent => ({ type: 'hostLocked', locked: index % 2 === 0 });

suite('EngineHost', () => {
	const disposables = new DisposableStore();
	let native: TestNativeEngine;
	let posted: unknown[];

	/** A host whose engine opened, as the utility process opens it. */
	async function openHost(): Promise<EngineHost> {
		const host = new EngineHost({ postMessage: message => posted.push(message) }, () => ({
			openEngine: () => Promise.resolve(native),
			openTemporaryEngine: () => Promise.resolve(native),
		}));
		await host.open();
		return host;
	}

	/** Listens to `host`'s events and returns the batches it delivers. */
	function listen(host: EngineHost): EngineEventBatch[] {
		const batches: EngineEventBatch[] = [];
		disposables.add(host.onDidReceiveEvents(batch => batches.push(batch)));
		return batches;
	}
	setup(() => {
		native = new TestNativeEngine();
		posted = [];
	});
	teardown(() => {
		disposables.clear();
	});
	suite('opening', () => {
		test('an opened engine is reported ready to main', async () => {
			await openHost();
			assert.deepStrictEqual(posted, [{ ready: true }]);
		});
		test('an engine that fails to open tells main why, then exits', async () => {
			const exits: number[] = [];
			const host = new EngineHost({ postMessage: message => posted.push(message) }, () => ({
				openEngine: () => Promise.reject(new Error('CorruptRecord')),
				openTemporaryEngine: () => Promise.reject(new Error('unused')),
			}), code => exits.push(code));
			await host.open();
			assert.deepStrictEqual(posted, [{ ready: false, error: 'CorruptRecord' }]);
			assert.deepStrictEqual(exits, [1]);
		});
	});
	suite('events', () => {
		test('nothing is read or watched before anyone listens', async () => {
			await openHost();
			await settled();
			assert.deepStrictEqual(native.commands, []);
			assert.deepStrictEqual(native.watches, []);
		});
		test('the first batch a listener receives asks it to resynchronize', async () => {
			const batches = listen(await openHost());
			await settled();
			assert.deepStrictEqual(batches, [{ events: [], resync: true }]);
		});
		test('an event is read when the engine wakes the host', async () => {
			const batches = listen(await openHost());
			await settled();
			native.queue.push({ type: 'hostLocked', locked: true });
			native.wake();
			await settled();
			assert.deepStrictEqual(batches.at(-1), { events: [{ type: 'hostLocked', locked: true }], resync: false });
		});
		test('events are not read again without a wake', async () => {
			listen(await openHost());
			await settled();
			const reads = native.reads();
			await settled();
			assert.strictEqual(native.reads(), reads);
		});
		test('a lagging subscription is reported as a resync', async () => {
			const batches = listen(await openHost());
			await settled();
			native.lagged = '3';
			native.wake();
			await settled();
			assert.deepStrictEqual(batches.at(-1), { events: [], resync: true });
		});
		test('removing the last listener closes the watch and releases the subscription', async () => {
			const host = await openHost();
			const listener = host.onDidReceiveEvents(() => undefined);
			await settled();
			listener.dispose();
			await settled();
			assert.deepStrictEqual(native.watches.map(watch => watch.closed), [true]);
			assert.deepStrictEqual(native.commands.filter(command => command.type === 'releaseEvents'), [{ type: 'releaseEvents', subscriptionId: '1' }]);
		});
		test('a read that fills its batch is followed by another', async () => {
			const batches = listen(await openHost());
			await settled();
			native.queue.push(...Array.from({ length: 300 }, (_, index) => locked(index)));
			native.wake();
			await settled();
			assert.deepStrictEqual(batches.slice(1).map(batch => batch.events.length), [256, 44]);
		});
		test('wakes during a read cost one more read, not one each', async () => {
			listen(await openHost());
			await settled();
			native.queue.push(locked(0));
			native.wake();
			native.wake();
			await settled();
			assert.strictEqual(native.reads(), 3);
		});
		test('a failed read produces no batch', async () => {
			const batches = listen(await openHost());
			await settled();
			native.failNextPoll = true;
			native.wake();
			await settled();
			assert.strictEqual(batches.length, 1);
		});
		test('a failed read is reported as a resync once the subscription is replaced', () => runWithFakedTimers({}, async () => {
			const batches = listen(await openHost());
			await settled();
			native.failNextPoll = true;
			native.wake();
			await settled();
			await timeout(500);
			await settled();
			assert.deepStrictEqual(batches, [{ events: [], resync: true }, { events: [], resync: true }]);
			assert.strictEqual(native.subscriptions, 2);
		}));
		test('a failed read is not retried once nobody listens', () => runWithFakedTimers({}, async () => {
			const host = await openHost();
			const listener = host.onDidReceiveEvents(() => undefined);
			await settled();
			native.failNextPoll = true;
			native.wake();
			await settled();
			listener.dispose();
			await settled();
			const commands = native.commands.length;
			await timeout(5000);
			assert.strictEqual(native.commands.length, commands);
		}));
	});
	suite('commands and connections', () => {
		test('a command that is not an object is refused', async () => {
			const host = await openHost();
			await assert.rejects(host.execute('snapshot'), /Invalid engine command/);
		});
		test('an invalid data-channel request closes its port', async () => {
			const host = await openHost();
			const port = new TestPort();
			assert.strictEqual(host.handledClientConnection({ data: { type: 'attachDataChannel', owner: 'window', request: { requestId: 'bad' } }, ports: [port] }), true);
			assert.strictEqual(port.closed, true);
		});
		test('main\'s connection message reaches the IPC server', async () => {
			const host = await openHost();
			assert.strictEqual(host.handledClientConnection({ data: ENGINE_HOST_CONNECTION, ports: [new TestPort()] }), false);
		});
		test('any other message opens no connection', async () => {
			const host = await openHost();
			const port = new TestPort();
			assert.strictEqual(host.handledClientConnection({ data: 'something else', ports: [port] }), true);
			assert.strictEqual(port.closed, false);
		});
		test('after shutdown, commands are refused', async () => {
			const host = await openHost();
			await host.execute({ type: 'shutdown' });
			await assert.rejects(host.execute({ type: 'snapshot' }), /shutting down/);
		});
		test('after shutdown, a data channel is refused and its port closed', async () => {
			const host = await openHost();
			await host.execute({ type: 'shutdown' });
			const port = new TestPort();
			host.handledClientConnection({ data: { type: 'attachDataChannel', owner: 'window', request: { requestId: 'late', kind: 'video', sessionId: '1', generation: '1' } }, ports: [port] });
			assert.strictEqual(port.closed, true);
		});
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

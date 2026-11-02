// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { IpcRendererEvent } from 'electron';
import { EventEmitter } from 'events';
import { timeout } from '../../../../common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../test/common/utils.js';
import { runWithFakedTimers } from '../../../../test/common/virtualScheduling/index.js';
import { createPreloadGlobals, type IPreloadIpcRenderer } from '../../electron-browser/preloadGlobals.js';

const channel = 'vnilink:dataChannelPort';

/** Runs `body` on virtual time, so the twenty-second timers of its acquisitions end with the test. */
function onVirtualTime(body: () => void): Promise<void> {
	return runWithFakedTimers({}, () => {
		body();
		return Promise.resolve();
	});
}

/** The preload's `ipcRenderer`, with main's messages delivered by the test. */
class TestIpcRenderer implements IPreloadIpcRenderer {
	public readonly sent: (readonly [string, readonly unknown[]])[] = [];
	private readonly emitter = new EventEmitter();

	public send(sendChannel: string, ...args: unknown[]): void {
		this.sent.push([sendChannel, args]);
	}

	public on(onChannel: string, listener: (event: IpcRendererEvent, ...args: unknown[]) => void): this {
		this.emitter.on(onChannel, listener);
		return this;
	}

	public once(onceChannel: string, listener: (event: IpcRendererEvent, ...args: unknown[]) => void): this {
		this.emitter.once(onceChannel, listener);
		return this;
	}

	public removeListener(removeChannel: string, listener: (event: IpcRendererEvent, ...args: unknown[]) => void): this {
		this.emitter.removeListener(removeChannel, listener);
		return this;
	}

	/** Delivers a message from main, with Electron's event carrying its sender and `ports`. */
	public deliver(deliverChannel: string, ports: MessagePort[], ...args: unknown[]): void {
		this.emitter.emit(deliverChannel, { sender: this, ports }, ...args);
	}
}

suite('Preload globals', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	let ipcRenderer: TestIpcRenderer;
	/** Ports handed to the page, under their nonce. */
	let posted: { nonce: string; ports: readonly MessagePort[] }[];
	/** The names of the ports the preload closed. */
	let closed: string[];
	let dropped: File[];
	function createGlobals() {
		return createPreloadGlobals({
			ipcRenderer,
			getPathForFile: file => {
				dropped.push(file);
				return '/dropped';
			},
			postPorts: (nonce, ports) => {
				posted.push({ nonce, ports });
			},
		}, {
			configuration: () => ({ windowId: 1 }),
			resolveConfiguration: () => Promise.resolve({ windowId: 1 }),
		});
	}

	/** A port main sends, named so the test can tell which ones were closed. */
	function port(name: string): MessagePort {
		const { port1 } = new MessageChannel();
		port1.close = () => {
			closed.push(name);
		};
		return port1;
	}
	setup(() => {
		ipcRenderer = new TestIpcRenderer();
		posted = [];
		closed = [];
		dropped = [];
	});
	test('the page gets only the IPC, the port relay, process details, file paths and its configuration', () => {
		assert.deepStrictEqual(Object.keys(createGlobals()).sort(), ['context', 'ipcMessagePort', 'ipcRenderer', 'process', 'webUtils']);
	});
	test('a dropped file gives the page its path and nothing else', () => {
		const globals = createGlobals();
		const file = new File([], 'notes.txt');
		assert.deepStrictEqual(Object.keys(globals.webUtils), ['getPathForFile']);
		assert.strictEqual(globals.webUtils.getPathForFile(file), '/dropped');
		assert.deepStrictEqual(dropped, [file]);
	});
	test('the page sees no environment', () => {
		assert.deepStrictEqual(createGlobals().process.env, {});
	});
	test('the page reads the configuration main resolved for its window', async () => {
		assert.strictEqual((await createGlobals().context.resolveConfiguration()).windowId, 1);
	});
	test('the page invokes nothing; only the preload asks for its configuration', () => {
		assert.strictEqual(Reflect.get(createGlobals().ipcRenderer, 'invoke'), undefined);
	});
	test('the page sends only on VniLink channels', () => {
		const globals = createGlobals();
		assert.throws(() => {
			globals.ipcRenderer.send('other', 1);
		}, /Unsupported/);
		globals.ipcRenderer.send('vnilink:message', 1);
		assert.deepStrictEqual(ipcRenderer.sent, [['vnilink:message', [1]]]);
	});
	test('a port is acquired only on a VniLink channel', () => {
		assert.throws(() => {
			createGlobals().ipcMessagePort.acquire('dataChannelPort', 'one');
		}, /Unsupported/);
	});
	test('a nonce of anything but letters, digits and dashes is refused', () => {
		assert.throws(() => {
			createGlobals().ipcMessagePort.acquire(channel, 'not a nonce!');
		}, /refused/);
	});
	test('a nonce that already waits is refused', () => onVirtualTime(() => {
		const globals = createGlobals();
		globals.ipcMessagePort.acquire(channel, 'one');
		assert.throws(() => {
			globals.ipcMessagePort.acquire(channel, 'one');
		}, /refused/);
	}));
	test('the port main sends for an acquired nonce is posted to the page under that nonce', () => onVirtualTime(() => {
		const globals = createGlobals();
		globals.ipcMessagePort.acquire(channel, 'one');
		const answer = port('answer');
		ipcRenderer.deliver(channel, [answer], 'one');
		assert.deepStrictEqual(posted, [{ nonce: 'one', ports: [answer] }]);
		assert.deepStrictEqual(closed, []);
	}));
	test('a port sent before any acquisition reaches nothing', () => onVirtualTime(() => {
		const globals = createGlobals();
		ipcRenderer.deliver(channel, [port('early')], 'unknown');
		globals.ipcMessagePort.acquire(channel, 'one');
		assert.deepStrictEqual(posted, []);
	}));
	test('a port the window did not ask for is closed, not posted', () => onVirtualTime(() => {
		const globals = createGlobals();
		globals.ipcMessagePort.acquire(channel, 'one');
		ipcRenderer.deliver(channel, [port('unsolicited')], 'other');
		assert.deepStrictEqual(closed, ['unsolicited']);
		assert.deepStrictEqual(posted, []);
	}));
	test('a second port for an answered nonce is closed, not posted', () => onVirtualTime(() => {
		const globals = createGlobals();
		globals.ipcMessagePort.acquire(channel, 'one');
		ipcRenderer.deliver(channel, [port('answer')], 'one');
		ipcRenderer.deliver(channel, [port('replay')], 'one');
		assert.deepStrictEqual(closed, ['replay']);
		assert.strictEqual(posted.length, 1);
	}));
	test('at most eight acquisitions wait at once', () => onVirtualTime(() => {
		const globals = createGlobals();
		for (let index = 0; index < 8; index++) {
			globals.ipcMessagePort.acquire(channel, `pending-${String(index)}`);
		}
		assert.throws(() => {
			globals.ipcMessagePort.acquire(channel, 'overflow');
		}, /refused/);
	}));
	test('a released acquisition makes room for another', () => onVirtualTime(() => {
		const globals = createGlobals();
		for (let index = 0; index < 8; index++) {
			globals.ipcMessagePort.acquire(channel, `pending-${String(index)}`);
		}
		globals.ipcMessagePort.release(channel, 'pending-0');
		globals.ipcMessagePort.acquire(channel, 'after-release');
	}));
	test('an acquisition main never answers is forgotten after twenty seconds', () => runWithFakedTimers({}, async () => {
		const globals = createGlobals();
		globals.ipcMessagePort.acquire(channel, 'one');
		await timeout(20_000);
		ipcRenderer.deliver(channel, [port('late')], 'one');
		assert.deepStrictEqual(closed, ['late']);
		assert.deepStrictEqual(posted, []);
	}));
	test('page listeners get a message\'s arguments, never Electron\'s event with its sender and ports', () => {
		const received: { event: object; args: unknown[] }[] = [];
		createGlobals().ipcRenderer.on('vnilink:message', (event, ...args) => {
			received.push({ event, args });
		});
		ipcRenderer.deliver('vnilink:message', [port('carried')], 'payload');
		assert.deepStrictEqual(received, [{ event: {}, args: ['payload'] }]);
	});
	test('a removed page listener hears nothing more', () => {
		const globals = createGlobals();
		let heard = 0;
		const listener = (): void => {
			heard++;
		};
		globals.ipcRenderer.on('vnilink:message', listener);
		ipcRenderer.deliver('vnilink:message', [], 'before');
		globals.ipcRenderer.removeListener('vnilink:message', listener);
		ipcRenderer.deliver('vnilink:message', [], 'after');
		assert.strictEqual(heard, 1);
	});
});

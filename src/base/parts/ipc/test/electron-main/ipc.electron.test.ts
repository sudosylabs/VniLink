// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { WebContents } from 'electron';
import { EventEmitter } from 'events';
import { timeout } from '../../../../common/async.js';
import { errorHandler, setUnexpectedErrorHandler } from '../../../../common/errors.js';
import { toDisposable } from '../../../../common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../test/common/utils.js';
import { Server } from '../../electron-main/ipc.electron.js';
import { ValidatedIpcMain, WORKBENCH_URL } from '../../electron-main/ipcMain.js';
import { TestIpcMain } from './testIpcMain.js';

/** Web contents that show the workbench in their main frame, and that a test destroys. */
class TestWebContents extends EventEmitter {
	public readonly mainFrame = { url: WORKBENCH_URL, parent: null };

	constructor(public readonly id: number) {
		super();
	}

	public send(): void {
		// The window's side of the connection is not under test.
	}
}

suite('Electron IPC server', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let ipc: TestIpcMain;
	let window: TestWebContents;
	let unexpected: unknown[];
	let previousErrorHandler: (error: unknown) => void;

	/** A server on `ipc`, whose application windows have the id of their contents plus 1000. */
	function createServer(): Server {
		const windowOf = (contents: WebContents) => ipc.isApplicationWindow(contents) ? { id: contents.id + 1000 } : null;
		return disposables.add(new Server(new ValidatedIpcMain(ipc), windowOf));
	}

	/** The window says hello and sends its first message, which the server replaces with the window's context. */
	async function connect(): Promise<void> {
		// Electron's event, from the window's main frame.
		const event = { sender: window, senderFrame: window.mainFrame };
		ipc.emit('vnilink:hello', event);
		ipc.emit('vnilink:message', event, Buffer.from([0]));
		await timeout(0);
	}
	setup(() => {
		ipc = new TestIpcMain();
		window = new TestWebContents(90);
		ipc.windows.add(window);
		unexpected = [];
		previousErrorHandler = errorHandler.getUnexpectedErrorHandler();
		setUnexpectedErrorHandler(error => unexpected.push(error));
		// A connection lives until its window says goodbye or goes away, as in the application.
		disposables.add(toDisposable(() => ipc.emit('vnilink:disconnect', { sender: window, senderFrame: window.mainFrame }, null)));
	});
	teardown(() => {
		setUnexpectedErrorHandler(previousErrorHandler);
	});
	test('a window that says hello connects', async () => {
		const server = createServer();
		await connect();
		assert.strictEqual(server.connections.length, 1);
	});
	test('a connection\'s context names the sender\'s window, whatever the window sent', async () => {
		const server = createServer();
		await connect();
		assert.strictEqual(server.connections[0]?.ctx, 'window:1090');
	});
	test('web contents that are not an application window do not connect', async () => {
		const server = createServer();
		ipc.windows.clear();
		await connect();
		assert.strictEqual(server.connections.length, 0);
		assert.strictEqual(unexpected.length, 1, 'its refused hello was not reported');
	});
	test('a window whose contents are destroyed leaves the server, though it never said goodbye', async () => {
		const server = createServer();
		await connect();
		window.emit('destroyed');
		// A closed window would otherwise keep its connection and every event subscription with it.
		assert.strictEqual(server.connections.length, 0);
	});
});

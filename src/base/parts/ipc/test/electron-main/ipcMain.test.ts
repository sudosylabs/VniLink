// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { IpcMainInvokeEvent, WebContents, WebFrameMain } from 'electron';
import { errorHandler, setUnexpectedErrorHandler } from '../../../../common/errors.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../test/common/utils.js';
import { ValidatedIpcMain, WORKBENCH_URL } from '../../electron-main/ipcMain.js';
import { TestIpcMain } from './testIpcMain.js';
import { upcastPartial } from '../../../../test/common/mock.js';

function frame(url: string | undefined, parent: WebFrameMain | null = null): WebFrameMain {
	return upcastPartial<WebFrameMain>({ url, parent });
}

/** Web contents whose main frame shows the workbench. */
function webContents(): WebContents {
	return upcastPartial<WebContents>({ id: 4, mainFrame: frame(WORKBENCH_URL) });
}

/** The event Electron passes with a message from `sender`, by default from its main frame. */
function ipcEvent(sender: WebContents, senderFrame: WebFrameMain | null = sender.mainFrame): IpcMainInvokeEvent {
	return upcastPartial<IpcMainInvokeEvent>({ sender, senderFrame });
}

/** Sender frames of an application window that are not its workbench main frame. */
const refusedFrames: readonly (readonly [string, () => WebFrameMain])[] = [
	['a blank page', () => frame('about:blank')],
	['a frame with no URL', () => frame(undefined)],
	['a subframe showing the workbench', () => frame(WORKBENCH_URL, frame(WORKBENCH_URL))],
	['another application page', () => frame('vnilink://app/other.html')],
	['a frame other than the main frame, showing the workbench', () => frame(WORKBENCH_URL)],
];

suite('ValidatedIpcMain', () => {
	let ipc: TestIpcMain;
	let window: WebContents;
	let calls: number;
	let unexpected: unknown[];
	let previousErrorHandler: (error: unknown) => void;

	/** Registers a privileged handler that counts the calls reaching it. */
	function handleProbe(channel = 'vnilink:probe'): void {
		new ValidatedIpcMain(ipc).handle(channel, () => {
			calls++;
			return Promise.resolve();
		});
	}
	setup(() => {
		ipc = new TestIpcMain();
		window = webContents();
		ipc.windows.add(window);
		calls = 0;
		unexpected = [];
		previousErrorHandler = errorHandler.getUnexpectedErrorHandler();
		// A refused message is reported, not thrown, as the application's handler does.
		setUnexpectedErrorHandler(error => unexpected.push(error));
	});
	teardown(() => {
		setUnexpectedErrorHandler(previousErrorHandler);
	});
	test('the workbench main frame of an application window reaches a privileged handler', async () => {
		handleProbe();
		await ipc.invoke('vnilink:probe', ipcEvent(window));
		assert.strictEqual(calls, 1);
		assert.deepStrictEqual(unexpected, []);
	});
	for (const [name, senderFrame] of refusedFrames) {
		test(`${name} is refused and reported, and never reaches the handler`, async () => {
			handleProbe();
			await assert.rejects(Promise.resolve(ipc.invoke('vnilink:probe', ipcEvent(window, senderFrame()))), /Invalid channel/);
			assert.strictEqual(calls, 0);
			assert.strictEqual(unexpected.length, 1);
		});
	}
	test('web contents that are not an application window are refused and reported', async () => {
		handleProbe();
		await assert.rejects(Promise.resolve(ipc.invoke('vnilink:probe', ipcEvent(webContents()))), /Invalid channel/);
		assert.strictEqual(calls, 0);
		assert.strictEqual(unexpected.length, 1);
	});
	test('a channel outside VniLink\'s is refused and reported', async () => {
		handleProbe('other:probe');
		await assert.rejects(Promise.resolve(ipc.invoke('other:probe', ipcEvent(window))), /Invalid channel/);
		assert.strictEqual(calls, 0);
		assert.strictEqual(unexpected.length, 1);
	});
	test('a listener hears messages only from the workbench main frame of an application window', () => {
		const heard: unknown[] = [];
		new ValidatedIpcMain(ipc).on('vnilink:message', (_event, message) => heard.push(message));
		ipc.emit('vnilink:message', ipcEvent(window), 'from the workbench');
		ipc.emit('vnilink:message', ipcEvent(window, frame('about:blank')), 'from a blank page');
		ipc.emit('vnilink:message', ipcEvent(webContents()), 'from other contents');
		assert.deepStrictEqual(heard, ['from the workbench']);
		assert.strictEqual(unexpected.length, 2);
	});
	test('a removed listener hears nothing more', () => {
		const validated = new ValidatedIpcMain(ipc);
		const heard: unknown[] = [];
		const listener = (_event: unknown, message: unknown): void => {
			heard.push(message);
		};
		validated.on('vnilink:message', listener);
		validated.removeListener('vnilink:message', listener);
		ipc.emit('vnilink:message', ipcEvent(window), 'after');
		assert.deepStrictEqual(heard, []);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

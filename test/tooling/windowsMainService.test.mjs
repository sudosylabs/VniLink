// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

/** A window whose page load the test finishes, recording when it is shown. */
const electron = `
import { EventEmitter } from 'node:events';
export const windows = [];
export class BrowserWindow extends EventEmitter {
	constructor() { super(); this.id = windows.length + 1; this.shown = 0; this.webContents = new EventEmitter(); windows.push(this); }
	loadURL() { return new Promise(resolve => { this.finishLoad = resolve; }); }
	isDestroyed() { return false; }
	isMinimized() { return false; }
	show() { this.shown++; }
	focus() { }
	removeMenu() { }
}
export const nativeTheme = { shouldUseDarkColors: false };
export const ipcMain = { on() { }, handle() { } };
export default { ipcMain, BrowserWindow };`;

const output = await build({
	stdin: { contents: `export { WindowsMainService } from './src/platform/windows/electron-main/windowsMainService.ts'; export { windows } from 'electron';`, loader: 'ts', resolveDir: process.cwd() },
	bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json', banner: { js: "const __dirname = '/app';" }, plugins: [{
		name: 'electron', setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: electron, loader: 'js' }));
		}
	}],
});

const { WindowsMainService, windows } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

test('a window asked for while its page loads is shown once the page is there, not blank before it', async () => {
	// Linux windows take their icon from Electron's resources folder, which plain Node does not have.
	process.resourcesPath ??= '/resources';
	const service = new WindowsMainService({ error() { }, warn() { }, info() { } });
	const opened = service.openMainWindow(false);
	const window = windows.at(-1);
	service.focusMainWindow();
	assert.equal(window.shown, 0, 'The window was shown before its page loaded');
	window.finishLoad();
	await opened;
	assert.equal(window.shown, 1, 'A request made while loading was dropped');
	service.focusMainWindow();
	assert.equal(window.shown, 2);
	service.dispose();
});

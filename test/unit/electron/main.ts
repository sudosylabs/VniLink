// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The Electron main process of `test:unit:electron`, after VS Code's
 * `test/unit/electron/index.js`: one window, hidden on macOS, runs the renderer unit
 * tests (`renderer.ts`) and reports each result here, where it is printed;
 * the process exits 0 only when every test passed. `build/lib/unitTests.ts`
 * passes the bundles and options in `VNILINK_UNIT_*`.
 */
import { app, BrowserWindow, ipcMain } from 'electron';
import { join } from 'node:path';
import { isMacintosh } from '../../../src/base/common/platform.js';

interface TestResult {
	readonly title: string;
	readonly state: 'passed' | 'failed' | 'pending';
	readonly duration?: number;
	readonly error?: string;
}

const output = process.env.VNILINK_UNIT_OUTPUT;

const profile = process.env.VNILINK_TEST_PROFILE;

if (!output || !profile) {
	throw new Error('test:unit:electron starts this process; run node build/run.ts test:unit:electron');
}
// Tests never share the development application's profile, settings or secrets.
app.setPath('userData', profile);

let failures = 0;

let finished = false;

ipcMain.on('vnilink:unit:result', (_event, result: TestResult) => {
	if (result.state === 'passed') {
		console.log(`  ✔ ${result.title}${result.duration !== undefined && result.duration > 75 ? ` (${String(result.duration)}ms)` : ''}`);
	} else if (result.state === 'pending') {
		console.log(`  - ${result.title}`);
	} else {
		failures++;
		console.log(`  ✖ ${result.title}\n${(result.error ?? '').replace(/^/gm, '      ')}`);
	}
});

ipcMain.on('vnilink:unit:end', (_event, summary: { passes: number; failures: number; pending: number }) => {
	finished = true;
	console.log(`\n  ${String(summary.passes)} passing, ${String(summary.failures)} failing, ${String(summary.pending)} pending`);
	app.exit(summary.failures === 0 && failures === 0 ? 0 : 1);
});

app.on('window-all-closed', () => {
	if (!finished) {
		console.error('The unit test window closed before the tests finished.');
		app.exit(1);
	}
});

// The tests wait on animation frames and timers. Chromium slows both for a window that is not
// seen, as this one is not; the window renders as if seen, as the smoke launcher's window does
// (`test/automation/src/application.ts`).
for (const flag of ['disable-backgrounding-occluded-windows', 'disable-renderer-backgrounding', 'disable-background-timer-throttling']) {
	app.commandLine.appendSwitch(flag);
}

void app.whenReady().then(async () => {
	const window = new BrowserWindow({
		show: false,
		width: 1200,
		height: 800,
		// As VS Code's unit runner: tests load Node's modules (`assert`) and report through IPC.
		webPreferences: { nodeIntegration: true, contextIsolation: false, sandbox: false, backgroundThrottling: false },
	});
	window.webContents.on('render-process-gone', (_event, details) => {
		console.error(`The unit test renderer stopped: ${details.reason}`);
		app.exit(1);
	});
	window.webContents.on('console-message', event => {
		if (event.level === 'error') {
			console.error(event.message);
		}
	});
	// Windows gives a window never shown about one frame a second, whatever the switches say, and
	// Linux one never mapped none, so the tests that wait on frames run out of time. Only macOS
	// draws a hidden window; elsewhere it is shown, without focus.
	if (!isMacintosh) {
		window.showInactive();
	}
	await window.loadFile(join(output, 'test/unit/electron/renderer.html'));
});

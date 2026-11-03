// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import { appendFileSync } from 'node:fs';
import { join } from 'node:path';
import { app, type BrowserWindow } from 'electron';
import Mocha from 'mocha';
import { NOTICE_URL } from '../../../../../src/platform/systemActions/common/notice.js';
import { windowPrompt } from '../../../../../src/platform/systemActions/electron-main/noticeWindow.js';
import type { Automation } from '../../../../automation/src/automation.js';
import { Notice } from '../../../../automation/src/notice.js';
import { restoresWindow, setNativeTheme } from '../../utils.js';

/** The next window main opens and shows, as the notice it opens. */
function nextNotice(): Promise<Notice> {
	return new Promise((resolve, reject) => {
		const timer = setTimeout(() => {
			app.off('browser-window-created', created);
			reject(new Error('The notice window did not open'));
		}, 5000);
		function created(_event: Electron.Event, window: BrowserWindow): void {
			app.off('browser-window-created', created);
			// Main shows the notice once its page has loaded and been told what to say.
			window.once('show', () => {
				clearTimeout(timer);
				resolve(new Notice(window));
			});
		}
		app.on('browser-window-created', created);
	});
}

/** The system action notice: its countdown, its backdrop, and how it ends. */
export function setup(automation: Automation): void {
	Mocha.describe('notice', () => {
		restoresWindow(automation);
		// The notice takes focus with Cancel focused, so a key typed in another
		// application while the suite runs would answer it. Only these tests answer.
		const ownInput = (_event: Electron.Event, window: BrowserWindow): void => {
			window.webContents.on('before-input-event', event => {
				event.preventDefault();
			});
		};
		Mocha.before(() => app.on('browser-window-created', ownInput));
		Mocha.after(() => app.off('browser-window-created', ownInput));
		for (const mode of ['light', 'dark'] as const) {
			Mocha.it(`counts down in the ${mode} theme over the window's backdrop and answers Cancel`, async () => {
				await setNativeTheme(automation, mode);
				const deadline = Date.now() + 30_000;
				const opened = nextNotice();
				const answer = windowPrompt({ action: 'restart', name: 'Studio Mac', deadline }, new AbortController().signal);
				const notice = await opened;
				assert.equal(notice.window.webContents.getURL(), NOTICE_URL);
				assert.equal(await notice.title(), 'Studio Mac wants to restart this computer');
				assert.equal(await notice.focused(), 'Cancel', 'Cancel is not focused');
				// The workbench's backdrop, or none where the system cannot draw one.
				const { material, root, body } = await notice.backdrop();
				assert.match(material, /^(none|vibrancy|glass|mica)$/);
				// Over glass or Mica nothing of the page's own paints behind the notice.
				if (material === 'glass' || material === 'mica') {
					assert.deepEqual([root, body], ['rgba(0, 0, 0, 0)', 'rgba(0, 0, 0, 0)'], `the page hides the ${material} backdrop`);
				}
				appendFileSync(join(automation.application.output, 'notice.log'), `${mode}: ${material} ${root} ${body}\n`);
				const first = await notice.detail();
				// The seconds left are those to the deadline when the notice was drawn,
				// however long the window took to open on this machine.
				const readAt = Date.now();
				const shown = /^Restarting in (\d+) s\. Save your work, or cancel\.$/.exec(first);
				assert(shown, `unexpected countdown: ${first}`);
				const left = Number(shown[1]);
				const remaining = deadline - readAt;
				assert(left <= 30 && left >= Math.floor(remaining / 1000), `the countdown shows ${String(left)} s with ${String(remaining)} ms left`);
				// A second later the countdown says one less; a bound, not a pause.
				await notice.waitForDetailChange(first, 3000);
				await notice.screenshot(join(automation.application.output, `notice-${mode}.png`));
				await notice.cancel();
				assert.equal(await answer, 0, 'Cancel did not answer');
				assert(notice.window.isDestroyed(), 'the answered notice stayed open');
			});
		}
		Mocha.it('closes and answers nothing when its request ends another way', async () => {
			const controller = new AbortController();
			const opened = nextNotice();
			const answer = windowPrompt({ action: 'shutdown', name: 'Studio Mac', deadline: Date.now() + 30_000 }, controller.signal);
			const notice = await opened;
			controller.abort();
			assert.equal(await answer, undefined);
			assert(notice.window.isDestroyed(), 'an aborted notice stayed open');
		});
	});
}

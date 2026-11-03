// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import Mocha from 'mocha';
import type { Automation } from '../../../../automation/src/automation.js';
import { restoresWindow, setNativeTheme } from '../../utils.js';

/**
 * The views the unit suites test, laid out in the real window and
 * photographed in each theme and at the sizes where their layout changes.
 */
export function setup(automation: Automation): void {
	Mocha.describe('context-menu', () => {
		restoresWindow(automation);
		for (const mode of ['light', 'dark'] as const) {
			Mocha.it(`opens in the ${mode} theme and closes on Escape`, async () => {
				await setNativeTheme(automation, mode);
				await automation.contextMenu.show();
				await automation.screenshot(`context-menu-${mode}`);
				await automation.contextMenu.dismiss();
			});
		}
	});
	Mocha.describe('data-channels', () => {
		Mocha.it('refuses a channel to an unknown session and keeps the engine answering', async () => {
			await (await automation.driver.fixtures()).run('data-channels');
		});
	});
	Mocha.describe('dialogs', () => {
		restoresWindow(automation);
		for (const mode of ['light', 'dark'] as const) {
			Mocha.it(`draws the restart confirmation in the ${mode} theme and closes it on Escape`, async () => {
				await setNativeTheme(automation, mode);
				await automation.dialogs.show();
				await automation.screenshot(`dialogs-${mode}`);
				await automation.dialogs.dismiss();
			});
		}
	});
	Mocha.describe('files', () => {
		restoresWindow(automation);
		Mocha.after(() => automation.files.clear());
		for (const [mode, width, height] of [['light', 1280, 760], ['dark', 1280, 760], ['dark', 700, 760]] as const) {
			const name = width < 760 ? 'files-narrow' : `files-${mode}`;
			Mocha.it(`shows a task mid-way in the ${mode} theme at ${String(width)} px without scrolling sideways`, async () => {
				await setNativeTheme(automation, mode);
				await automation.files.show(width, height);
				await automation.screenshot(name);
				assert.equal(await automation.files.overflows(), false, `the Files tab scrolls sideways at ${String(width)} px`);
			});
		}
	});
	Mocha.describe('remote-desktop-session', () => {
		restoresWindow(automation);
		Mocha.before(() => {
			automation.window.setSize(1400, 900);
		});
		Mocha.after(() => automation.remoteDesktop.clear());
		for (const state of ['collapsed', 'details', 'slow', 'bar'] as const) {
			Mocha.it(`shows the desktop with its controls ${state}`, async () => {
				await automation.remoteDesktop.show(state);
				await automation.screenshot(`desktop-${state}`);
			});
		}
	});
}

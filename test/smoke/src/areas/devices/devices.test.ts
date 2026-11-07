// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import type { DownloadItem } from 'electron';
import Mocha from 'mocha';
import type { Automation } from '../../../../automation/src/automation.js';
import { restoresWindow, setNativeTheme } from '../../utils.js';

/** Resolves with the name of the next download the window starts, cancelled before it writes. */
function nextDownload(automation: Automation): Promise<string> {
	const session = automation.window.webContents.session;
	return new Promise<string>((resolve, reject) => {
		const timer = setTimeout(() => {
			session.removeListener('will-download', listener);
			reject(new Error('Invitation download did not start'));
		}, 5000);
		const listener = (_event: Electron.Event, item: DownloadItem): void => {
			clearTimeout(timer);
			item.cancel();
			resolve(item.getFilename());
		};
		session.once('will-download', listener);
	});
}

/** The Devices page: its inspector at each size, and pairing in both directions. */
export function setup(automation: Automation): void {
	Mocha.describe('devices', () => {
		restoresWindow(automation);
		Mocha.after(() => automation.devices.clear());
		for (const mode of ['light', 'dark'] as const) {
			Mocha.it(`shows a selected computer in the ${mode} theme`, async () => {
				await setNativeTheme(automation, mode);
				automation.window.setSize(1400, 900);
				await automation.devices.show('selected');
				const selection = await automation.devices.selection();
				assert.strictEqual(selection.rows.length, 1, 'one computer is selected');
				assert.ok(selection.rows[0].includes(selection.inspector), `the inspector shows ${selection.inspector}, not the selected computer`);
				await automation.screenshot(`devices-selected-${mode}`);
			});
		}
		Mocha.it('shows the inspector as a full-height sheet that scrolls on its own', async () => {
			// A short window makes the inspector's content taller than the sheet.
			automation.window.setSize(1400, 600);
			await automation.devices.show('selected');
			const sheet = await automation.devices.inspector();
			const { scrolls, listStays, pageScrolls, fullHeight, atRightEdge } = sheet;
			assert.deepEqual(
				{ scrolls, listStays, pageScrolls, fullHeight, atRightEdge },
				{ scrolls: true, listStays: true, pageScrolls: false, fullHeight: true, atRightEdge: true },
				'the inspector is not a full-height sheet with its own scroll',
			);
		});
		Mocha.it('covers the list with the inspector in a narrow window, without scrolling sideways', async () => {
			automation.window.setSize(800, 800);
			await automation.devices.show('selected');
			assert.equal(await automation.devices.overflows(), false, 'the device panel overflows a narrow window');
			const sheet = await automation.devices.inspector();
			assert.deepEqual({ overList: sheet.overList, atRightEdge: sheet.atRightEdge }, { overList: true, atRightEdge: true }, 'a narrow window split the page instead of showing the inspector as a sheet');
			await automation.screenshot('devices-selected-narrow');
		});
		for (const [state, title] of [['empty', 'Bring your computers together'], ['unavailable', 'Devices unavailable']] as const) {
			Mocha.it(`shows the page ${state}`, async () => {
				await automation.devices.show(state);
				await automation.devices.waitForEmptyTitle(title);
				await automation.screenshot(`devices-${state}`);
			});
		}
		Mocha.it('creates an invitation, shows its code and downloads it through Chromium', async () => {
			const download = nextDownload(automation);
			assert.equal(await automation.devices.shareInvitation(), '042517');
			assert.equal(await download, 'VniLink.vnl');
			await automation.screenshot('devices-pairing-popover');
		});
		Mocha.it('pairs a computer that asks once its digits are typed, without showing the expected code', async () => {
			await automation.devices.approveIncoming('731904');
			await automation.devices.waitForTitle('Computer paired');
			await automation.screenshot('devices-pairing-paired');
		});
		Mocha.it('opens an invitation, takes the creator\'s digits and shows this computer\'s code', async () => {
			await automation.devices.startOpeningInvitation();
			await automation.screenshot('devices-import-choose');
			await automation.devices.openInvitation('04251');
			await automation.screenshot('devices-import-code');
			// The fixture's engine answers the creator's 042517 with this computer's 918273.
			assert.strictEqual(await automation.devices.finishInvitation('7'), '918273');
			await automation.screenshot('devices-import-confirm');
		});
	});
}

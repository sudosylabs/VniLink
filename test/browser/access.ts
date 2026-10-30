// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { isMacintosh } from '../../src/base/common/platform.js';
import { assert, settle, SettingsFixture, toggle } from './settingsFixture.js';

/** Sharing and remote action rules apply per switch; OS access is live state with its own actions. */
export async function verifyAccess(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		fixture.configuration.setRestricted('security.allowRestart', true);
		const page = fixture.open('access');
		await settle();
		const view = page.element.querySelector<HTMLInputElement>('[data-setting="sharing.screen"]');
		assert(view && !view.disabled && !view.checked, 'Screen sharing switch missing');
		toggle(view);
		await settle();
		assert(fixture.configuration.updates.at(-1)?.key === 'sharing.screen' && view.checked, 'Screen sharing did not apply');
		fixture.configuration.failure = new Error('Sharing settings could not be changed.');
		toggle(view);
		await settle();
		assert(view.checked, 'Refused sharing change kept the refused value');
		assert(page.element.querySelector('.settings-status')?.textContent === 'Sharing settings could not be changed.', 'Refused sharing change was not explained');
		fixture.configuration.failure = undefined;
		const restart = page.element.querySelector<HTMLInputElement>('[data-setting="security.allowRestart"]');
		assert(restart?.disabled && restart.closest('.setting-row')?.textContent.includes('Not supported on this computer.'), 'Unsupported action was not marked');
		assert(page.element.querySelector<HTMLInputElement>('[data-setting="security.allowLock"]')?.checked, 'Remote lock default missing');
		const permissions = page.element.querySelector('[data-permission-kind="camera"]')?.closest('section');
		assert(permissions && !permissions.querySelector('input'), 'Permissions section shows a switch');
		assert(!permissions.textContent.includes('Development builds'), 'Permissions retained development-only copy');
		const camera = permissions.querySelector<HTMLButtonElement>('[data-permission-kind="camera"] button');
		assert(camera?.textContent === 'Request access', 'Camera request missing');
		assert(permissions.querySelector('[data-permission-kind="screen"] button')?.textContent === (isMacintosh ? 'Open System Settings' : 'Open Settings'), 'Screen settings action missing');
		camera.click();
		await settle();
		assert(permissions.querySelector('[data-permission-kind="camera"] [data-status="granted"]'), 'Camera request did not refresh');
		fixture.permissions.fail = true;
		fixture.permissions.focused.fire();
		await settle();
		assert(fixture.configuration.reloads > 0, 'Focus did not read the engine settings again');
		assert(permissions.querySelector<HTMLButtonElement>('button')?.disabled, 'Failed check left actions enabled');
		const commands = fixture.permissions.commands.length;
		page.dispose();
		fixture.permissions.focused.fire();
		await settle();
		assert(fixture.permissions.commands.length === commands, 'Disposed page kept checking OS access');
	} finally {
		fixture.dispose();
	}
}

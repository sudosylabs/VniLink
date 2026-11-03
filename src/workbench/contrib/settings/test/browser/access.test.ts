// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { isMacintosh } from '../../../../../base/common/platform.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import type { IHomePage } from '../../../home/homePages.js';
import { click, element, SettingsFixture, settle, toggle } from './settingsFixture.js';

suite('Access settings page', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: SettingsFixture;
	let page: IHomePage;
	setup(async () => {
		fixture = disposables.add(new SettingsFixture());
		fixture.configuration.setRestricted('security.allowRestart', true);
		page = fixture.open('access');
		await settle();
	});
	const screen = (): HTMLInputElement => element<HTMLInputElement>(page.element, '[data-setting="sharing.screen"]');
	const permissions = (): HTMLElement => {
		const section = element(page.element, '[data-permission-kind="camera"]').closest('section');
		assert.ok(section, 'the permissions are not in a section');
		return section;
	};
	test('screen sharing is off by default and applies when switched on', async () => {
		assert.deepStrictEqual({ disabled: screen().disabled, checked: screen().checked }, { disabled: false, checked: false });
		toggle(screen());
		await settle();
		assert.strictEqual(fixture.configuration.updates.at(-1)?.key, 'sharing.screen');
		assert.strictEqual(screen().checked, true);
	});
	test('a refused sharing change keeps the value in effect and explains why', async () => {
		toggle(screen());
		await settle();
		fixture.configuration.failure = new Error('Sharing settings could not be changed.');
		toggle(screen());
		await settle();
		assert.strictEqual(screen().checked, true);
		assert.strictEqual(page.element.querySelector('.settings-status')?.textContent, 'Sharing settings could not be changed.');
	});
	test('an action this computer does not support is disabled and marked', () => {
		const restart = element<HTMLInputElement>(page.element, '[data-setting="security.allowRestart"]');
		assert.strictEqual(restart.disabled, true);
		assert.ok(restart.closest('.setting-row')?.textContent.includes('Not supported on this computer.'));
	});
	test('remote lock is allowed by default', () => {
		assert.strictEqual(element<HTMLInputElement>(page.element, '[data-setting="security.allowLock"]').checked, true);
	});
	test('operating-system access is shown as live state, without switches or development-only copy', () => {
		assert.strictEqual(permissions().querySelector('input'), null);
		assert.ok(!permissions().textContent.includes('Development builds'));
	});
	test('the camera offers a request and the screen opens the system settings', () => {
		assert.strictEqual(permissions().querySelector('[data-permission-kind="camera"] button')?.textContent, 'Request access');
		assert.strictEqual(permissions().querySelector('[data-permission-kind="screen"] button')?.textContent, isMacintosh ? 'Open System Settings' : 'Open Settings');
	});
	test('requesting the camera shows it granted', async () => {
		click(element<HTMLButtonElement>(permissions(), '[data-permission-kind="camera"] button'));
		await settle();
		assert.ok(permissions().querySelector('[data-permission-kind="camera"] [data-status="granted"]'));
	});
	test('gaining focus reads the engine settings again', async () => {
		fixture.permissions.focus();
		await settle();
		assert.ok(fixture.configuration.reloads > 0);
	});
	test('a failed access check disables its actions', async () => {
		const section = permissions();
		fixture.permissions.fail = true;
		fixture.permissions.focus();
		await settle();
		assert.strictEqual(section.querySelector<HTMLButtonElement>('button')?.disabled, true);
	});
	test('a disposed page stops checking operating-system access', async () => {
		const commands = fixture.permissions.commands.length;
		page.dispose();
		fixture.permissions.focus();
		await settle();
		assert.strictEqual(fixture.permissions.commands.length, commands);
	});
});

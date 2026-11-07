// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { click, edit, element, SettingsFixture, settle } from './settingsFixture.js';

suite('Media settings page', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: SettingsFixture;
	let page: HTMLElement;
	let cameras: HTMLSelectElement;
	setup(async () => {
		fixture = disposables.add(new SettingsFixture());
		page = fixture.open('media').element;
		await settle();
		cameras = element<HTMLSelectElement>(page, 'select[data-setting="media.camera"]');
	});

	/** The camera's row, read again: a request re-renders it. */
	const cameraRow = (): HTMLElement => {
		const row = cameras.closest<HTMLElement>('.setting-row');
		assert.ok(row, 'the camera picker is not in a setting row');
		return row;
	};
	const offered = (): HTMLButtonElement | null => cameraRow().querySelector<HTMLButtonElement>('button.settings-button');
	const request = (): HTMLButtonElement => element<HTMLButtonElement>(cameraRow(), 'button.settings-button');
	test('each media source has a picker', () => {
		assert.deepStrictEqual(Array.from(page.querySelectorAll<HTMLSelectElement>('select'), select => select.dataset.setting), ['media.camera', 'media.microphone', 'media.systemAudio']);
	});
	test('the camera picker offers every camera, the default and this computer\'s cameras', () => {
		assert.deepStrictEqual(Array.from(cameras.options).map(option => option.value), ['vnilink:all-cameras', '', 'camera-one']);
	});
	test('a device name is shown as text', () => {
		assert.ok(Array.from(cameras.options).some(option => option.textContent === 'Camera <one>'));
	});
	test('a camera choice applies', async () => {
		edit(cameras, 'camera-one');
		await settle();
		assert.strictEqual(fixture.configuration.getValue<unknown>('media.camera'), 'camera-one');
	});
	test('a saved microphone that is missing is shown as unavailable', () => {
		fixture.configuration.set('media.microphone', 'gone');
		assert.strictEqual(element<HTMLSelectElement>(page, 'select[data-setting="media.microphone"]').selectedOptions.item(0)?.textContent, 'Selected device (unavailable)');
	});
	test('camera access is asked for in the camera\'s row, with the reason', () => {
		assert.strictEqual(request().hidden, false);
		assert.strictEqual(request().textContent, 'Allow');
		assert.ok(cameraRow().textContent.includes('Camera access isn’t allowed yet.'));
	});
	test('Allow requests camera access', async () => {
		click(request());
		await settle();
		assert.ok(fixture.permissions.commands.some(command => command.type === 'request' && command.kind === 'camera'));
	});
	test('granted camera access is no longer offered or explained', async () => {
		click(request());
		await settle();
		assert.notStrictEqual(offered()?.hidden, false);
		assert.ok(!cameraRow().textContent.includes('isn’t allowed'));
	});
});

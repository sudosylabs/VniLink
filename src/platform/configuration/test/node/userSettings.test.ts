// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DisposableStore } from '../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { settingDefaults } from '../../common/configurationSchema.js';
import { UserSettings } from '../../node/userSettings.js';

suite('UserSettings', () => {
	const disposables = new DisposableStore();
	let dir: string;
	let path: string;

	/** Settings read from `path`, as main opens them. */
	async function open(): Promise<UserSettings> {
		const settings = disposables.add(new UserSettings(path));
		await settings.initialize();
		return settings;
	}
	const saved = async (): Promise<unknown> => JSON.parse(await readFile(path, 'utf8'));
	setup(async () => {
		dir = await mkdtemp(join(tmpdir(), 'vnilink-settings-'));
		path = join(dir, 'settings.json');
	});
	teardown(async () => {
		disposables.clear();
		await rm(dir, { recursive: true, force: true });
	});
	suite('a readable file', () => {
		setup(async () => {
			await writeFile(path, JSON.stringify({ 'future.setting': { kept: true } }));
		});
		test('a file of unknown keys reads as the defaults', async () => {
			const settings = await open();
			assert.deepStrictEqual(settings.values, settingDefaults('user'));
		});
		test('only changed settings are written, beside the unknown keys', async () => {
			const settings = await open();
			let changes = 0;
			disposables.add(settings.onDidChange(() => changes++));
			await Promise.all([settings.update('appearance.theme', 'dark'), settings.update('input.swapModifiers', 'always')]);
			assert.strictEqual(changes, 2);
			assert.deepStrictEqual(await saved(), { 'future.setting': { kept: true }, 'appearance.theme': 'dark', 'input.swapModifiers': 'always' });
		});
		test('a setting changed back to its default leaves the file', async () => {
			const settings = await open();
			await settings.update('appearance.theme', 'dark');
			await settings.update('input.swapModifiers', 'always');
			await settings.update('input.swapModifiers', 'automatic');
			assert.deepStrictEqual(await saved(), { 'future.setting': { kept: true }, 'appearance.theme': 'dark' });
		});
		test('writing leaves no temporary file behind', async () => {
			const settings = await open();
			await settings.update('appearance.theme', 'dark');
			assert.deepStrictEqual(await readdir(dir), ['settings.json']);
		});
		test('a caller changing a value after handing it over does not change what is saved', async () => {
			const settings = await open();
			const quality = { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 };
			const writing = settings.update('desktop.quality', quality);
			quality.max_frame_rate = 1;
			await writing;
			assert.strictEqual((settings.values['desktop.quality'] as typeof quality).max_frame_rate, 30);
		});
		test('the values a caller reads are frozen', async () => {
			const settings = await open();
			await settings.update('desktop.quality', { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 });
			assert.ok(Object.isFrozen(settings.values['desktop.quality']));
		});
		test('saved settings survive reopening', async () => {
			const settings = await open();
			await settings.update('appearance.theme', 'dark');
			await settings.update('desktop.quality', { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 });
			const reopened = await open();
			assert.strictEqual(reopened.values['appearance.theme'], 'dark');
			assert.strictEqual((reopened.values['desktop.quality'] as { max_bitrate_kbps: number }).max_bitrate_kbps, 5000);
		});
	});
	for (const [name, contents] of [
		['broken JSON', '{broken'],
		['an invalid value', '{"appearance.theme":"unknown"}'],
		['an engine setting', '{"power.lockAfterControl":true}'],
		['an array', '[]'],
	] as const) {
		suite(`a file holding ${name}`, () => {
			setup(async () => {
				await writeFile(path, contents);
			});
			test('reports an error and uses the defaults', async () => {
				const settings = await open();
				assert.ok(settings.error);
				assert.deepStrictEqual(settings.values, settingDefaults('user'));
			});
			test('refuses updates and is never overwritten', async () => {
				const settings = await open();
				await assert.rejects(settings.update('appearance.theme', 'light'));
				assert.strictEqual(await readFile(path, 'utf8'), contents);
			});
		});
	}
	suite('a write that fails', () => {
		let settings: UserSettings;
		let changes: number;
		setup(async () => {
			settings = await open();
			// A folder in the file's place makes the atomic rename fail.
			await mkdir(path);
			changes = 0;
			disposables.add(settings.onDidChange(() => changes++));
		});
		test('keeps the effective value, announces nothing and leaves no temporary file', async () => {
			await assert.rejects(settings.update('appearance.theme', 'dark'));
			assert.strictEqual(changes, 0);
			assert.strictEqual(settings.values['appearance.theme'], 'system');
			assert.deepStrictEqual(await readdir(dir), ['settings.json']);
		});
		test('does not stop a later write from succeeding', async () => {
			await assert.rejects(settings.update('appearance.theme', 'dark'));
			await rm(path, { recursive: true });
			await settings.update('appearance.theme', 'light');
			assert.strictEqual(changes, 1);
			assert.strictEqual(settings.values['appearance.theme'], 'light');
		});
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

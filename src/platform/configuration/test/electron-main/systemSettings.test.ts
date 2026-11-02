// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { mkdtemp, readFile, rm, stat } from 'fs/promises';
import { tmpdir } from 'os';
import { join } from 'path';
import { DisposableStore } from '../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { AutostartLoginItem, autostartEntry, SystemSettings } from '../../electron-main/systemSettings.js';

type LoginItem = NonNullable<ConstructorParameters<typeof SystemSettings>[0]>;

/** The operating system's login item, recording each write. */
class TestLoginItem implements LoginItem {
	public readonly writes: boolean[] = [];

	constructor(public readonly supported: boolean, private enabled = false) { }

	public read(): Promise<boolean> {
		return Promise.resolve(this.enabled);
	}

	public write(enabled: boolean): Promise<void> {
		this.writes.push(enabled);
		this.enabled = enabled;
		return Promise.resolve();
	}
}

suite('SystemSettings', () => {
	const disposables = new DisposableStore();
	/** Whether a tray icon can show. */
	let trayShows: boolean;
	setup(() => {
		trayShows = true;
	});
	teardown(() => {
		disposables.clear();
	});
	async function createSettings(item: TestLoginItem): Promise<SystemSettings> {
		const settings = disposables.add(new SystemSettings(item, () => Promise.resolve(trayShows)));
		await settings.refresh();
		return settings;
	}
	function changes(settings: SystemSettings): () => number {
		let count = 0;
		disposables.add(settings.onDidChange(() => count++));
		return () => count;
	}
	test('start at login shows the operating system\'s state', async () => {
		const settings = await createSettings(new TestLoginItem(true, true));
		assert.deepStrictEqual(settings.values, { 'application.openAtLogin': true });
	});
	test('start at login can be changed where this launch can register', async () => {
		const settings = await createSettings(new TestLoginItem(true, true));
		assert.strictEqual(settings.restricted.includes('application.openAtLogin'), false);
	});
	test('changing start at login writes the login item', async () => {
		const item = new TestLoginItem(true, true);
		const settings = await createSettings(item);
		await settings.update('application.openAtLogin', false);
		assert.deepStrictEqual(item.writes, [false]);
		assert.deepStrictEqual(settings.values, { 'application.openAtLogin': false });
	});
	test('changing start at login announces the change once', async () => {
		const settings = await createSettings(new TestLoginItem(true, true));
		const count = changes(settings);
		await settings.update('application.openAtLogin', false);
		assert.strictEqual(count(), 1);
	});
	test('running in the background is not changed here', async () => {
		const settings = await createSettings(new TestLoginItem(true));
		await assert.rejects(settings.update('application.runInBackground', true));
	});
	test('a launch that cannot register reports start at login off, whatever the login item says', async () => {
		const settings = await createSettings(new TestLoginItem(false, true));
		assert.deepStrictEqual(settings.values, { 'application.openAtLogin': false });
	});
	test('a launch that cannot register restricts start at login', async () => {
		const settings = await createSettings(new TestLoginItem(false, true));
		assert.ok(settings.restricted.includes('application.openAtLogin'));
	});
	test('a launch that cannot register refuses to change start at login', async () => {
		const item = new TestLoginItem(false, true);
		const settings = await createSettings(item);
		await assert.rejects(settings.update('application.openAtLogin', true));
		assert.deepStrictEqual(item.writes, []);
	});
	test('running in the background is restricted while no tray icon can show', async () => {
		trayShows = false;
		const settings = await createSettings(new TestLoginItem(true));
		assert.ok(settings.restricted.includes('application.runInBackground'));
	});
	test('running in the background is offered once a tray icon can show', async () => {
		trayShows = false;
		const settings = await createSettings(new TestLoginItem(true));
		trayShows = true;
		await settings.refresh();
		assert.strictEqual(settings.restricted.includes('application.runInBackground'), false);
	});
	test('a tray host appearing announces the settings again', async () => {
		trayShows = false;
		const settings = await createSettings(new TestLoginItem(true));
		const count = changes(settings);
		trayShows = true;
		await settings.refresh();
		assert.strictEqual(count(), 1);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

suite('autostartEntry', () => {
	test('refuses an executable path with a line break', () => {
		assert.throws(() => autostartEntry('/opt/vni\nlink'));
	});
	test('quotes the executable and escapes what the Exec key and its strings treat specially', () => {
		const entry = autostartEntry('/opt/Vni "Link"/$HOME/50%/a\\b');
		assert.ok(entry.split('\n').includes(String.raw`Exec="/opt/Vni \\"Link\\"/\\$HOME/50%%/a\\\\b" --hidden`), entry);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

suite('AutostartLoginItem', () => {
	let directory: string;
	let path: string;
	setup(async () => {
		directory = await mkdtemp(join(tmpdir(), 'vnilink-autostart-'));
		path = join(directory, 'autostart', 'vnilink.desktop');
	});
	teardown(async () => {
		await rm(directory, { recursive: true, force: true });
	});
	test('reads a missing entry as disabled', async () => {
		assert.strictEqual(await new AutostartLoginItem(path, '/opt/VniLink/vnilink').read(), false);
	});
	test('enabling writes an entry that starts the executable hidden', async () => {
		const item = new AutostartLoginItem(path, '/opt/VniLink/vnilink');
		await item.write(true);
		assert.strictEqual(await item.read(), true);
		assert.ok((await readFile(path, 'utf8')).split('\n').includes('Exec="/opt/VniLink/vnilink" --hidden'));
	});
	test('enabling leaves no partly written file behind', async () => {
		await new AutostartLoginItem(path, '/opt/VniLink/vnilink').write(true);
		await assert.rejects(stat(`${path}.tmp`));
	});
	test('disabling removes the entry', async () => {
		const item = new AutostartLoginItem(path, '/opt/VniLink/vnilink');
		await item.write(true);
		await item.write(false);
		assert.strictEqual(await item.read(), false);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

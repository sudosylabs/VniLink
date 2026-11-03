// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { NullLogService } from '../../../log/common/log.js';
import { UIStateChannel } from '../../electron-main/uiStateChannel.js';
import { SaveStrategy, StateService } from '../../node/stateService.js';

const lastFolder = 'files.lastFolder.k51qzi5uqu5d-studio_2.remote';

const folderValue = { root: '0', path: 'Users/someone/Documents' };

/** Keys a window may neither read nor write: the main process's own, unknown and malformed ones. */
const refusedKeys: readonly unknown[] = ['windowsState', 'files.lastFolder.studio.both', 'files.lastFolder..local', 'files.lastFolder.a/b.local', `files.lastFolder.${'x'.repeat(129)}.local`, 'files.showHidden.extra', 42];

const refusedShowHidden: readonly unknown[] = ['yes', 1, null, undefined, { root: '0', path: 'a', extra: 1 }];

const refusedFolders: readonly unknown[] = [true, '/Users', { root: '', path: 'a' }, { root: 1, path: 'a' }, { root: '0' }, { root: '0', path: 'a'.repeat(4097) }, { root: '0', path: 'a', extra: 1 }];

suite('UIStateChannel', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let folder: string;
	let path: string;
	let state: StateService;
	let channel: UIStateChannel;
	let opened: StateService[];

	/** The state file at `path`, as main opens it at startup. */
	async function openState(): Promise<StateService> {
		const service = disposables.add(new StateService(SaveStrategy.DELAYED, path, new NullLogService()));
		opened.push(service);
		await service.init();
		return service;
	}
	setup(async () => {
		folder = await mkdtemp(join(tmpdir(), 'vnilink-state-'));
		path = join(folder, 'storage.json');
		opened = [];
		state = await openState();
		state.setItem('windowsState', { main: true });
		channel = new UIStateChannel(state);
	});
	teardown(async () => {
		await rm(folder, { recursive: true, force: true });
	});

	/**
	 * A test that closes the state files it opened as it ends, saving what is
	 * pending, before the leak check disposes them: disposing first would
	 * cancel the save.
	 */
	function stateTest(name: string, body: () => Promise<void> | void): void {
		test(name, async () => {
			try {
				await body();
			} finally {
				await Promise.all(opened.map(service => service.close()));
			}
		});
	}
	stateTest('a call that does not come from an application window is refused', async () => {
		await assert.rejects(channel.call('renderer', 'set', { key: 'files.showHidden', value: true }), /window/i);
	});
	stateTest('a window remembers the last folder of a computer\'s pane', async () => {
		await channel.call('window:1', 'set', { key: lastFolder, value: folderValue });
		assert.deepStrictEqual(await channel.call('window:1', 'get', { key: lastFolder }), folderValue);
	});
	stateTest('a window remembers whether hidden files show', async () => {
		await channel.call('window:1', 'set', { key: 'files.showHidden', value: true });
		assert.strictEqual(await channel.call('window:1', 'get', { key: 'files.showHidden' }), true);
	});
	stateTest('a folder never saved reads as nothing', async () => {
		await channel.call('window:1', 'set', { key: lastFolder, value: folderValue });
		assert.strictEqual(await channel.call('window:1', 'get', { key: 'files.lastFolder.other.local' }), undefined);
	});
	stateTest('a remote desktop remembers, for each computer, whether it exchanges Control and Command', async () => {
		await channel.call('window:1', 'set', { key: 'desktop.swapModifiers.studio', value: false });
		assert.strictEqual(await channel.call('window:1', 'get', { key: 'desktop.swapModifiers.studio' }), false);
	});
	stateTest('a modifier exchange that is not a boolean is refused', async () => {
		await assert.rejects(channel.call('window:1', 'set', { key: 'desktop.swapModifiers.studio', value: 'off' }), /Invalid UI state value/);
	});
	for (const key of refusedKeys) {
		stateTest(`the key ${String(key)} cannot be read`, async () => {
			await assert.rejects(channel.call('window:1', 'get', { key }), /Invalid UI state key/);
		});
		stateTest(`the key ${String(key)} cannot be written`, async () => {
			await assert.rejects(channel.call('window:1', 'set', { key, value: true }), /Invalid UI state value/);
			assert.deepStrictEqual(state.getItem('windowsState'), { main: true }, 'a refused write changed the state file');
		});
	}
	for (const value of refusedShowHidden) {
		stateTest(`whether hidden files show cannot be ${value === undefined ? 'undefined' : JSON.stringify(value)}`, async () => {
			await assert.rejects(channel.call('window:1', 'set', { key: 'files.showHidden', value }), /Invalid UI state value/);
			assert.strictEqual(state.getItem('files.showHidden'), undefined);
		});
	}
	for (const value of refusedFolders) {
		stateTest(`a last folder cannot be ${JSON.stringify(value)}`, async () => {
			await assert.rejects(channel.call('window:1', 'set', { key: lastFolder, value }), /Invalid UI state value/);
			assert.strictEqual(state.getItem(lastFolder), undefined);
		});
	}
	stateTest('a read that carries a value is refused', async () => {
		await assert.rejects(channel.call('window:1', 'get', { key: lastFolder, value: 1 }), /Invalid UI state key/);
	});
	stateTest('an unknown command is refused', async () => {
		await assert.rejects(channel.call('window:1', 'clear', {}), /Unknown UI state command/);
	});
	stateTest('the channel offers no events', () => {
		assert.throws(() => channel.listen('window:1', 'onDidChange'), /Unknown UI state event/);
	});
	stateTest('saved values survive a restart and a removed one is gone, leaving the main process\'s keys', async () => {
		await channel.call('window:1', 'set', { key: lastFolder, value: folderValue });
		await channel.call('window:1', 'set', { key: 'files.showHidden', value: true });
		await channel.call('window:1', 'remove', { key: 'files.showHidden' });
		await state.close();
		const reopened = await openState();
		assert.deepStrictEqual(reopened.getItem(lastFolder), folderValue);
		assert.strictEqual(reopened.getItem('files.showHidden'), undefined);
		assert.deepStrictEqual(reopened.getItem('windowsState'), { main: true });
	});
});

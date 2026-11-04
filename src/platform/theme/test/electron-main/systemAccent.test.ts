// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { SystemAccent } from '../../electron-main/systemAccent.js';
import { TestAccentPreferences } from './testAccentPreferences.js';

suite('SystemAccent', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let preferences: TestAccentPreferences;
	setup(() => {
		preferences = new TestAccentPreferences();
	});
	test('the system\'s accent is read as an opaque #rrggbb', () => {
		preferences.accent = 'FF5257FF';
		const accent = disposables.add(new SystemAccent(preferences, 'darwin'));
		assert.strictEqual(accent.color, '#ff5257');
	});
	test('a system without an accent leaves the product accent', () => {
		const accent = disposables.add(new SystemAccent(preferences, 'linux'));
		assert.strictEqual(accent.color, undefined);
		assert.strictEqual(preferences.subscriptions.size, 0, 'subscribed to a system that posts nothing');
	});
	test('an answer that is not a colour is no accent', () => {
		preferences.accent = '';
		assert.strictEqual(disposables.add(new SystemAccent(preferences, 'win32')).color, undefined);
		preferences.getAccentColor = () => {
			throw new Error('No accent');
		};
		assert.strictEqual(disposables.add(new SystemAccent(preferences, 'win32')).color, undefined);
	});
	test('macOS: choosing another accent is announced once, from either notification', () => {
		const accent = disposables.add(new SystemAccent(preferences, 'darwin'));
		const seen: (string | undefined)[] = [];
		disposables.add(accent.onDidChange(() => seen.push(accent.color)));
		preferences.accent = 'a550a7ff';
		preferences.post('AppleColorPreferencesChangedNotification');
		preferences.post('NSSystemColorsDidChangeNotification');
		assert.deepStrictEqual(seen, ['#a550a7']);
		assert.deepStrictEqual([...preferences.subscriptions.values()].map(({ event, local }) => [event, local]), [
			['AppleColorPreferencesChangedNotification', false],
			['NSSystemColorsDidChangeNotification', true],
		]);
	});
	test('Windows: a changed accent colour is announced', () => {
		const accent = disposables.add(new SystemAccent(preferences, 'win32'));
		const seen: (string | undefined)[] = [];
		disposables.add(accent.onDidChange(() => seen.push(accent.color)));
		preferences.accent = '0078d4ff';
		preferences.emit('accent-color-changed');
		assert.deepStrictEqual(seen, ['#0078d4']);
	});
	test('disposing stops following the system', () => {
		const accent = new SystemAccent(preferences, 'darwin');
		accent.dispose();
		assert.strictEqual(preferences.subscriptions.size, 0);
		const windows = new SystemAccent(preferences, 'win32');
		windows.dispose();
		assert.strictEqual(preferences.listenerCount('accent-color-changed'), 0);
	});
});

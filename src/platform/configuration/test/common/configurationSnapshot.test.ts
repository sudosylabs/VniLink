// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { settingDefaults } from '../../common/configurationSchema.js';
import { isConfigurationSnapshot, isConfigurationUpdate } from '../../common/configurationSnapshot.js';

suite('configurationSnapshot', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const snapshot = { revision: 1, values: settingDefaults('user'), errors: { power: 'Unavailable' }, restricted: ['security.allowRestart'] };
	test('a valid update is accepted', () => {
		assert.strictEqual(isConfigurationUpdate({ key: 'appearance.theme', value: 'dark' }), true);
	});
	for (const [name, update] of [
		['an invalid value', { key: 'appearance.theme', value: 'invalid' }],
		['an extra field', { key: 'appearance.theme', value: 'dark', extra: true }],
	] as const) {
		test(`an update with ${name} is refused`, () => {
			assert.strictEqual(isConfigurationUpdate(update), false);
		});
	}
	test('a snapshot of the defaults with a source error and a restriction is accepted', () => {
		assert.strictEqual(isConfigurationSnapshot(snapshot), true);
	});
	for (const [name, invalid] of [
		['a negative revision', { ...snapshot, revision: -1 }],
		['an error for an unknown source', { ...snapshot, errors: { future: 'x' } }],
		['a relay token', { ...snapshot, values: { 'connection.relays': [{ url: 'https://relay.example/', authenticated: true, replacementToken: 'secret' }] } }],
	] as const) {
		test(`a snapshot with ${name} is refused`, () => {
			assert.strictEqual(isConfigurationSnapshot(invalid), false);
		});
	}
	test('a snapshot with an authenticated relay but no token is accepted', () => {
		assert.strictEqual(isConfigurationSnapshot({ ...snapshot, values: { 'connection.relays': [{ url: 'https://relay.example/', authenticated: true }] } }), true);
	});
});

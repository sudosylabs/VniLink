// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { hidUsage } from '../../common/keyUsage.js';

suite('hidUsage', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const usages: [string, number][] = [
		['KeyA', 0x04], ['KeyZ', 0x1d], ['Digit1', 0x1e], ['Digit0', 0x27], ['F1', 0x3a], ['F13', 0x68], ['Numpad0', 0x62], ['MetaRight', 0xe7],
	];
	for (const [code, usage] of usages) {
		test(`the key at ${code} is HID usage 0x${usage.toString(16)}`, () => {
			assert.strictEqual(hidUsage(code), usage);
		});
	}
	test('only the table\'s own keys have a usage', () => {
		assert.strictEqual(hidUsage('constructor'), undefined);
	});
	test('a key with no HID usage has none', () => {
		assert.strictEqual(hidUsage('Fn'), undefined);
	});
});

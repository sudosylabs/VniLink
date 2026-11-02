// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { windowIdFromContext } from '../../electron-main/windowContext.js';

suite('windowIdFromContext', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('a window context names its window', () => {
		assert.strictEqual(windowIdFromContext('window:12'), 12);
	});
	for (const context of ['window:0', 'renderer']) {
		test(`the context '${context}' names no application window`, () => {
			assert.throws(() => windowIdFromContext(context), /application window/);
		});
	}
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { isMacintosh } from '../../../../base/common/platform.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { getNativeTitlebarHeight } from '../../common/window.js';

suite('getNativeTitlebarHeight', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	// The height follows the platform the test runs on; macOS reports its kernel version, 25 being Tahoe.
	test('a macOS Tahoe titlebar is 32 pixels high, other platforms 30', () => {
		assert.strictEqual(getNativeTitlebarHeight('25.0.0'), isMacintosh ? 32 : 30);
	});
	test('a macOS titlebar before Tahoe is 28 pixels high, other platforms 30', () => {
		assert.strictEqual(getNativeTitlebarHeight('24.6.0'), isMacintosh ? 28 : 30);
	});
});

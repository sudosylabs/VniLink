// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isPictureName } from '../../common/recording.js';

suite('isPictureName', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('a PNG file name is a picture name', () => {
		assert.strictEqual(isPictureName('Studio Mac camera 2026-10-19 14.32.05.png'), true);
	});
	const refused: readonly (readonly [string, string])[] = [
		['a path climbing out of the folder', '../x.png'],
		['a path into a folder', 'a/b.png'],
		['a Windows drive path', 'C:\\x.png'],
		['an extension alone', '.png'],
		['a hidden file', '.hidden.png'],
		['a JPEG', 'picture.jpg'],
		['a name holding a NUL', 'a\0.png'],
		['a name longer than 200 characters', `${'a'.repeat(200)}.png`],
	];
	for (const [name, value] of refused) {
		test(`${name} is not a picture name`, () => {
			assert.strictEqual(isPictureName(value), false);
		});
	}
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { resolveWindowMaterial, type WindowBackgroundMaterial } from '../../common/windowMaterial.js';

/** Each platform and kernel release, with the backdrop it supports. */
const releases: readonly (readonly [platform: string, release: string, material: WindowBackgroundMaterial])[] = [
	['darwin', '27.0.0', 'glass'],
	['darwin', '25.0.0', 'glass'],
	['darwin', '24.6.0', 'vibrancy'],
	['darwin', '23.0.0', 'vibrancy'],
	['win32', '10.0.22621', 'mica'],
	['win32', '10.0.22000', 'none'],
	['win32', '10.0.19045', 'none'],
	['linux', '6.8.0', 'none'],
	['darwin', 'invalid', 'none'],
];

suite('resolveWindowMaterial', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	for (const [platform, release, material] of releases) {
		test(`${platform} ${release} uses ${material}`, () => {
			assert.strictEqual(resolveWindowMaterial(platform, release, false, false), material);
		});
	}
	test('a release with glass is opaque when transparency is reduced', () => {
		assert.strictEqual(resolveWindowMaterial('darwin', '27.0.0', true, false), 'none');
	});
	test('a release with mica is opaque in high contrast', () => {
		assert.strictEqual(resolveWindowMaterial('win32', '10.0.22621', false, true), 'none');
	});
});

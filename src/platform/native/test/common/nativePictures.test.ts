// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { VSBuffer } from '../../../../base/common/buffer.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isNativeHostRequest, isPng } from '../../common/native.js';

suite('Native host pictures', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const png = VSBuffer.wrap(Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0]));
	test('a buffer starting with the PNG signature is a PNG', () => {
		assert.strictEqual(isPng(png), true);
	});
	test('a JPEG is not a PNG', () => {
		assert.strictEqual(isPng(VSBuffer.wrap(Uint8Array.from([0xff, 0xd8, 0xff, 0, 0, 0, 0, 0, 0]))), false);
	});
	test('PNG bytes that do not arrive as a buffer are refused', () => {
		assert.strictEqual(isPng(Uint8Array.from(png.buffer)), false);
	});
	test('a picture larger than a window may save is refused', () => {
		assert.strictEqual(isPng(VSBuffer.alloc(33 * 1024 * 1024)), false);
	});
	test('a window does not choose where a saved picture goes', () => {
		assert.strictEqual(isNativeHostRequest('savePicture', [3, 'Camera.png', png]), false);
	});
	test('a window may copy a PNG to the clipboard', () => {
		assert.strictEqual(isNativeHostRequest('writeClipboardPicture', [3, png]), true);
	});
	test('a window may not copy text as a clipboard picture', () => {
		assert.strictEqual(isNativeHostRequest('writeClipboardPicture', [3, 'text']), false);
	});
});

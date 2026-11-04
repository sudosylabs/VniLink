// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isNativeHostRequest } from '../../common/native.js';

const panel = { x: 8, y: 36, width: 224, height: 600, cornerRadius: 10 };

suite('Native host requests', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('a window may say whether its remote desktop has the keyboard', () => {
		assert.strictEqual(isNativeHostRequest('setRemoteKeyboard', [7, true]), true);
	});
	test('a window may place its material panel, or withdraw it', () => {
		assert.strictEqual(isNativeHostRequest('setMaterialPanel', [7, panel]), true);
		assert.strictEqual(isNativeHostRequest('setMaterialPanel', [7, undefined]), true);
		assert.strictEqual(isNativeHostRequest('setMaterialPanel', [7]), true);
	});
	const refused: readonly (readonly [string, string, readonly unknown[]])[] = [
		['full screen set to something other than a boolean', 'setFullScreen', [7, 'yes']],
		['a remote keyboard request that does not say whether it has the keyboard', 'setRemoteKeyboard', [7]],
		['window controls of infinite height', 'updateWindowControls', [7, { height: Infinity }]],
		['focusing another window', 'focusWindow', [7, { targetWindowId: 8 }]],
		['opening an empty link', 'openExternal', [7, '']],
		['a relaunch with arguments', 'relaunch', [7, true]],
		['a method the host does not serve', 'execute', [7]],
		['a material panel at an infinite place', 'setMaterialPanel', [7, { ...panel, x: Infinity }]],
		['a material panel of negative width', 'setMaterialPanel', [7, { ...panel, width: -1 }]],
		['a material panel larger than any window', 'setMaterialPanel', [7, { ...panel, height: 1e6 }]],
		['a material panel with a radius beyond its corners\' reach', 'setMaterialPanel', [7, { ...panel, cornerRadius: 65 }]],
		['a material panel without a radius', 'setMaterialPanel', [7, { x: 8, y: 36, width: 224, height: 600 }]],
		['a material panel carrying more than its rectangle', 'setMaterialPanel', [7, { ...panel, tintColor: '#fff' }]],
		['a material panel given as a string', 'setMaterialPanel', [7, '8,36,224,600']],
		['two material panels at once', 'setMaterialPanel', [7, panel, panel]],
	];
	for (const [name, method, args] of refused) {
		test(`refuses ${name}`, () => {
			assert.strictEqual(isNativeHostRequest(method, args), false);
		});
	}
});

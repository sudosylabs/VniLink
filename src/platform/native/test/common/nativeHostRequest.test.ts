// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isNativeHostRequest } from '../../common/native.js';

suite('Native host requests', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('a window may say whether its remote desktop has the keyboard', () => {
		assert.strictEqual(isNativeHostRequest('setRemoteKeyboard', [7, true]), true);
	});
	const refused: readonly (readonly [string, string, readonly unknown[]])[] = [
		['full screen set to something other than a boolean', 'setFullScreen', [7, 'yes']],
		['a remote keyboard request that does not say whether it has the keyboard', 'setRemoteKeyboard', [7]],
		['window controls of infinite height', 'updateWindowControls', [7, { height: Infinity }]],
		['focusing another window', 'focusWindow', [7, { targetWindowId: 8 }]],
		['opening an empty link', 'openExternal', [7, '']],
		['a relaunch with arguments', 'relaunch', [7, true]],
		['a method the host does not serve', 'execute', [7]],
	];
	for (const [name, method, args] of refused) {
		test(`refuses ${name}`, () => {
			assert.strictEqual(isNativeHostRequest(method, args), false);
		});
	}
});

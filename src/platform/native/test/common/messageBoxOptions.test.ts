// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isMessageBoxOptions } from '../../common/native.js';

/** Message boxes window 3 asks for, and whether main shows them. */
const cases: readonly (readonly [string, unknown, boolean])[] = [
	['a short warning with three plain buttons', { type: 'warning', message: 'Restart Studio PC?', detail: 'Unsaved work may be lost.', buttons: ['Restart', 'Shut Down', 'Cancel'], defaultId: 2, cancelId: 2 }, true],
	['five buttons', { message: 'x', buttons: ['1', '2', '3', '4', '5'] }, false],
	['a long message', { message: 'x'.repeat(501) }, false],
	['a cancel id past the buttons', { message: 'x', buttons: ['OK'], cancelId: 1 }, false],
	['a checkbox', { message: 'Replace notes.txt?', buttons: ['Replace', 'Skip'], checkboxLabel: 'Do the same for the other conflicts', checkboxChecked: true }, true],
	['an empty checkbox label', { message: 'x', checkboxLabel: '' }, false],
	['a long checkbox label', { message: 'x', checkboxLabel: 'x'.repeat(101) }, false],
	['a checkbox state that is not a boolean', { message: 'x', checkboxLabel: 'Always', checkboxChecked: 'yes' }, false],
	['a checkbox state without a checkbox', { message: 'x', checkboxChecked: true }, false],
	['an option the window may not set', { message: 'x', textWidth: 300 }, false],
	['another window', { message: 'x', targetWindowId: 4 }, false],
	['an error box', { message: 'x', type: 'error' }, false],
];

suite('isMessageBoxOptions', () => {
	for (const [name, options, shown] of cases) {
		test(`a window's message box is ${shown ? 'shown' : 'refused'}: ${name}`, () => {
			assert.strictEqual(isMessageBoxOptions(options, 3), shown);
		});
	}
	ensureNoDisposablesAreLeakedInTestSuite();
});

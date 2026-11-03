// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { $ } from '../../../base/browser/dom.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../base/test/common/utils.js';
import { Layout } from '../../layout.js';

/** A layout whose notice a test sets, as the workbench does. */
class TestLayout extends Layout {
	public show(message: string | undefined, action?: { readonly label: string; run(): void }): void {
		this.setNotice(message, action);
	}

	/** The notice line, which the layout keeps in its workspace until the parts are laid out. */
	public get noticeLine(): HTMLElement {
		return this.notice;
	}
}

suite('Layout', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	test('a notice with an action shows a button that runs it, and the next notice replaces both', () => {
		const layout = disposables.add(new TestLayout($('div')));
		let ran = 0;
		layout.show('Something needs you.', { label: 'Fix It…', run: () => ran++ });
		const button = layout.noticeLine.querySelector<HTMLElement>('.workbench-notice-action .monaco-button');
		assert.strictEqual(button?.textContent, 'Fix It…');
		assert.strictEqual(layout.noticeLine.querySelector('.workbench-notice-text')?.textContent, 'Something needs you.');
		button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
		assert.strictEqual(ran, 1);
		layout.show('Something else.');
		assert.strictEqual(layout.noticeLine.querySelector('.workbench-notice-action'), null, 'a notice without an action shows no button');
		layout.show(undefined);
		assert.strictEqual(layout.noticeLine.hidden, true);
	});
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import '../../../../../platform/theme/browser/media/colors.css';
import assert from 'assert';
import { $ } from '../../../../../base/browser/dom.js';
import { animationFrames } from '../../../../../base/test/browser/waits.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import type { IDialogOptions, IDialogResult } from '../../dialogService.js';
import type { DialogService } from '../../dialogServiceImpl.js';
import { conflictQuestion as conflict, createDialogService as dialogService, NativeDialogService, restartQuestion as restart, TestMessageBoxHost, ThemedDialogService } from './dialogFixture.js';

function pressEscape(): void {
	const target = document.activeElement ?? document.body;
	for (const type of ['keydown', 'keyup']) {
		target.dispatchEvent(new KeyboardEvent(type, { key: 'Escape', code: 'Escape', keyCode: 27, bubbles: true, cancelable: true }));
	}
}

/** Clicks `element` as the pointer does: the dialog listens for the click, not for a method call. */
function click(element: HTMLElement | null | undefined): void {
	assert.ok(element, 'nothing to click');
	element.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

function box(): HTMLElement | null {
	return document.querySelector<HTMLElement>('.monaco-dialog-box');
}

function button(label: string): HTMLElement | undefined {
	return [...document.querySelectorAll<HTMLElement>('.monaco-dialog-box .dialog-buttons .monaco-button')].find(candidate => candidate.textContent === label);
}

/** The colour `token` resolves to, as computed styles report it. */
function resolvedBackground(token: string): string {
	const probe = $('div');
	probe.style.backgroundColor = `var(${token})`;
	document.body.append(probe);
	const value = getComputedStyle(probe).backgroundColor;
	probe.remove();
	return value;
}

suite('DialogService', () => {
	suite('native', () => {
		test('returns the answer of the native box', async () => {
			const answer = await dialogService(NativeDialogService, new TestMessageBoxHost(0)).show(restart);
			assert.deepStrictEqual(answer, { button: 0, checked: false });
		});
		test('the native box defaults to and cancels with the cancel answer', async () => {
			const host = new TestMessageBoxHost(0);
			await dialogService(NativeDialogService, host).show(restart);
			assert.strictEqual(host.shown[0].defaultId, 2);
			assert.strictEqual(host.shown[0].cancelId, 2);
			assert.strictEqual(host.shown[0].message, restart.message);
		});
		test('a question without a checkbox asks the native box for none', async () => {
			const host = new TestMessageBoxHost(0);
			await dialogService(NativeDialogService, host).show(restart);
			assert.strictEqual('checkboxLabel' in host.shown[0], false);
			assert.strictEqual('checkboxChecked' in host.shown[0], false);
		});
		test('the native box draws the checkbox, unticked', async () => {
			const host = new TestMessageBoxHost(1, true);
			await dialogService(NativeDialogService, host).show(conflict);
			assert.strictEqual(host.shown[0].checkboxLabel, conflict.checkbox?.label);
			assert.strictEqual(host.shown[0].checkboxChecked, false);
		});
		test('a ticked native checkbox is reported with the answer', async () => {
			const answer = await dialogService(NativeDialogService, new TestMessageBoxHost(1, true)).show(conflict);
			assert.deepStrictEqual(answer, { button: 1, checked: true });
		});
	});
	suite('themed', () => {
		let host: TestMessageBoxHost;
		let service: DialogService;
		setup(() => {
			host = new TestMessageBoxHost(0);
			service = dialogService(ThemedDialogService, host);
		});
		teardown(async () => {
			// Answers any dialog a failed test left open, so the next starts on a clean page.
			if (box()) {
				pressEscape();
				await animationFrames(1);
			}
		});

		/** Shows `options` and waits for the dialog to be drawn; `answer` settles once it is answered. */
		async function show(options: IDialogOptions): Promise<{ readonly answer: Promise<IDialogResult> }> {
			const answer = service.show(options);
			await animationFrames(1);
			return { answer };
		}
		test('is a modal dialog', async () => {
			const { answer } = await show(restart);
			assert.strictEqual(box()?.getAttribute('role'), 'dialog');
			assert.strictEqual(box()?.getAttribute('aria-modal'), 'true');
			pressEscape();
			await answer;
		});
		test('opens no native box', async () => {
			const { answer } = await show(restart);
			assert.deepStrictEqual(host.shown, []);
			pressEscape();
			await answer;
		});
		test('shows the question and its detail', async () => {
			const { answer } = await show(restart);
			assert.strictEqual(box()?.querySelector('#monaco-dialog-message-text')?.textContent, restart.message);
			assert.strictEqual(box()?.querySelector('#monaco-dialog-message-detail')?.textContent, restart.detail);
			pressEscape();
			await answer;
		});
		test('offers every answer', async () => {
			const { answer } = await show(restart);
			const labels = [...document.querySelectorAll<HTMLElement>('.monaco-dialog-box .dialog-buttons .monaco-button')].map(candidate => candidate.textContent);
			assert.deepStrictEqual([...labels].sort(), [...restart.buttons].sort());
			pressEscape();
			await answer;
		});
		test('focuses the cancel answer', async () => {
			const { answer } = await show(restart);
			assert.strictEqual(document.activeElement?.textContent, 'Cancel');
			pressEscape();
			await answer;
		});
		test('draws its warning icon', async () => {
			const { answer } = await show(restart);
			assert.notStrictEqual(box()?.querySelector('.dialog-message-row svg'), null);
			pressEscape();
			await answer;
		});
		test('shows no close button, which has no glyph', async () => {
			const { answer } = await show(restart);
			assert.strictEqual(box()?.querySelector('.dialog-close-action'), null);
			pressEscape();
			await answer;
		});
		test('is drawn on the raised surface of the theme', async () => {
			const { answer } = await show(restart);
			const dialog = box();
			assert.ok(dialog);
			assert.notStrictEqual(resolvedBackground('--surface-raised'), 'rgba(0, 0, 0, 0)', 'the theme defines the raised surface');
			assert.strictEqual(getComputedStyle(dialog).backgroundColor, resolvedBackground('--surface-raised'));
			pressEscape();
			await answer;
		});
		test('Escape answers with the cancel answer', async () => {
			const { answer } = await show(restart);
			pressEscape();
			assert.deepStrictEqual(await answer, { button: 2, checked: false });
		});
		test('closes once answered', async () => {
			const { answer } = await show(restart);
			pressEscape();
			await answer;
			await animationFrames(1);
			assert.strictEqual(box(), null);
		});
		test('a click answers with its button', async () => {
			const { answer } = await show(restart);
			click(button('Shut Down'));
			assert.strictEqual((await answer).button, 1);
		});
		test('shows the checkbox with its label, unticked', async () => {
			const { answer } = await show(conflict);
			const row = document.querySelector<HTMLElement>('.monaco-dialog-box .dialog-checkbox-row');
			assert.ok(row);
			assert.strictEqual(row.querySelector('.dialog-checkbox-message')?.textContent, conflict.checkbox?.label);
			assert.strictEqual(row.querySelector('.monaco-checkbox')?.getAttribute('aria-checked'), 'false');
			pressEscape();
			await answer;
		});
		test('an unticked checkbox hides its tick', async () => {
			const { answer } = await show(conflict);
			const tick = document.querySelector<HTMLElement>('.monaco-dialog-box .monaco-checkbox .vnilink-icon');
			assert.ok(tick);
			assert.strictEqual(getComputedStyle(tick).visibility, 'hidden');
			pressEscape();
			await answer;
		});
		test('clicking the label ticks the checkbox and shows the product tick', async () => {
			const { answer } = await show(conflict);
			click(document.querySelector<HTMLElement>('.monaco-dialog-box .dialog-checkbox-message'));
			const checkbox = document.querySelector<HTMLElement>('.monaco-dialog-box .monaco-checkbox');
			const tick = checkbox?.querySelector<HTMLElement>('.vnilink-icon');
			assert.ok(checkbox && tick);
			assert.strictEqual(checkbox.getAttribute('aria-checked'), 'true');
			assert.strictEqual(getComputedStyle(tick).visibility, 'visible');
			assert.notStrictEqual(tick.querySelector('svg'), null);
			pressEscape();
			await answer;
		});
		test('a ticked checkbox is reported with the answer', async () => {
			const { answer } = await show(conflict);
			click(document.querySelector<HTMLElement>('.monaco-dialog-box .dialog-checkbox-message'));
			click(button('Keep Both'));
			assert.deepStrictEqual(await answer, { button: 1, checked: true });
		});
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

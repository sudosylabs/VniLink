// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../../base/common/async.js';
import { until } from '../../../../../base/test/browser/waits.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../../base/test/common/virtualScheduling/index.js';
import { DevicePageFixture } from './devicePageFixture.js';

/** How often a live invitation asks the engine for an incoming request. */
const requestPoll = 500;

function pressKey(target: Element, key: string, keyCode: number): void {
	target.dispatchEvent(new KeyboardEvent('keydown', { key, keyCode, bubbles: true }));
}

function click(element: Element | null | undefined): void {
	assert.ok(element, 'the element to click exists');
	element.dispatchEvent(new MouseEvent('click', { bubbles: true }));
}

/** Types `code` into a code input one digit at a time, as a keyboard does. */
function typeCode(boxes: readonly HTMLInputElement[], code: string): void {
	for (let index = 0; index < code.length; index++) {
		const box = boxes[index];
		box.focus();
		box.dispatchEvent(new InputEvent('beforeinput', { inputType: 'insertText', data: code.charAt(index), bubbles: true, cancelable: true }));
	}
}

function popover(): HTMLElement | null {
	return document.querySelector<HTMLElement>('.device-pairing-popover');
}

function text(selector: string): string {
	return popover()?.querySelector<HTMLElement>(`${selector}:not([hidden])`)?.textContent ?? '';
}

function heading(): string | undefined {
	return popover()?.querySelector('h2')?.textContent;
}

function primary(): HTMLButtonElement | null {
	return popover()?.querySelector<HTMLButtonElement>('.device-pairing-foot .monaco-button.primary:not([hidden])') ?? null;
}

function boxes(): HTMLInputElement[] {
	return [...popover()?.querySelectorAll<HTMLInputElement>('.device-pairing-field:not([hidden]) .code-input-digit') ?? []];
}

function step(): string {
	return text('.device-pairing-progress');
}

suite('DevicePairingPopover', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: DevicePageFixture;

	/**
	 * Each test runs in virtual time: the invitation's poll for an incoming
	 * request and its expiry countdown run when the test moves time, so a
	 * request is noticed after one poll interval rather than after waiting.
	 * The page is disposed before virtual time ends, which ends the poll.
	 */
	function pairingTest(title: string, body: () => Promise<void>): void {
		test(title, () => runWithFakedTimers({ maxTaskCount: 1_000 }, async () => {
			try {
				await body();
			} finally {
				fixture.dispose();
			}
		}));
	}

	/** Opens the invitation popover and waits for its code. */
	async function invite(): Promise<HTMLButtonElement> {
		const create = fixture.anchor('create');
		click(create);
		await timeout(0);
		return create;
	}

	/** Opens the invitation, then lets a request from another computer arrive. */
	async function approve(): Promise<void> {
		await invite();
		fixture.incoming = { endpoint_id: 'test-computer' };
		await timeout(requestPoll);
	}

	/** Pairs by approving an incoming request with its whole code. */
	async function paired(): Promise<void> {
		await approve();
		typeCode(boxes(), '731904');
		pressKey(boxes()[5], 'Enter', 13);
		await timeout(0);
	}

	/** Opens a dropped invitation file after a finished pairing. */
	async function dropInvitation(): Promise<HTMLElement | null> {
		await paired();
		document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
		click(fixture.anchor('import'));
		const importing = popover();
		const dropped = new DataTransfer();
		dropped.items.add(new File(['VNLF'], 'office-pc.vnl'));
		importing?.querySelector('h2')?.dispatchEvent(new DragEvent('drop', { dataTransfer: dropped, bubbles: true, cancelable: true }));
		// The page reads the file, which no virtual time can hurry.
		await until(() => boxes().length === 6, 'dropping an invitation asks for the creator\'s digits');
		return importing;
	}
	setup(() => {
		fixture = disposables.add(new DevicePageFixture('empty'));
	});
	pairingTest('inviting shows the invitation code', async () => {
		await invite();
		assert.strictEqual(text('.device-pairing-code'), '042517');
	});
	pairingTest('the invitation is a dialog labelled by its heading', async () => {
		await invite();
		const dialog = popover();
		assert.strictEqual(dialog?.getAttribute('role'), 'dialog');
		assert.strictEqual(dialog.getAttribute('aria-labelledby'), dialog.querySelector('h2')?.id);
		assert.strictEqual(heading(), 'Invite another computer');
	});
	pairingTest('the invitation is the first of two steps', async () => {
		await invite();
		assert.strictEqual(step(), '1 of 2');
	});
	pairingTest('the invitation says when it expires', async () => {
		await invite();
		assert.match(text('.device-pairing-expiry'), /^Expires in 1[45]:[0-5][0-9]$/);
	});
	pairingTest('the invitation offers Save… and takes focus', async () => {
		await invite();
		assert.strictEqual(primary()?.textContent, 'Save…');
		assert.ok(popover()?.contains(document.activeElement));
	});
	pairingTest('Escape closes the popover and returns focus to its button', async () => {
		const create = await invite();
		pressKey(primary() ?? document.body, 'Escape', 27);
		assert.strictEqual(popover(), null);
		assert.strictEqual(document.activeElement, create);
	});
	pairingTest('reopening shows the live invitation again', async () => {
		await invite();
		pressKey(primary() ?? document.body, 'Escape', 27);
		await invite();
		assert.strictEqual(text('.device-pairing-code'), '042517');
	});
	pairingTest('an incoming request asks for its digits', async () => {
		await approve();
		assert.strictEqual(boxes().length, 6);
		assert.strictEqual(heading(), 'Approve this computer');
	});
	pairingTest('approving is the second of two steps and does not disclose the code', async () => {
		await approve();
		assert.strictEqual(step(), '2 of 2');
		assert.strictEqual(text('.device-pairing-code'), '');
	});
	pairingTest('a lapsed request returns to the invitation and says why', async () => {
		await approve();
		fixture.incoming = null;
		await timeout(requestPoll);
		assert.strictEqual(text('.device-pairing-code'), '042517');
		assert.strictEqual(text('.device-feedback'), 'The other computer stopped waiting. It will ask again while this invitation lasts.');
	});
	pairingTest('a renewed request asks for its digits again', async () => {
		await approve();
		fixture.incoming = null;
		await timeout(requestPoll);
		fixture.incoming = { endpoint_id: 'test-computer' };
		await timeout(requestPoll);
		assert.strictEqual(boxes().length, 6);
	});
	pairingTest('approval focuses the first digit', async () => {
		await approve();
		assert.strictEqual(document.activeElement, boxes()[0]);
	});
	pairingTest('Approve is not offered before the whole code is typed', async () => {
		await approve();
		assert.strictEqual(primary()?.classList.contains('disabled'), true);
	});
	pairingTest('typing a digit moves to the next box', async () => {
		await approve();
		typeCode(boxes(), '73');
		assert.strictEqual(document.activeElement, boxes()[2]);
	});
	pairingTest('Backspace in an empty box returns to the previous digit and clears it', async () => {
		await approve();
		typeCode(boxes(), '73');
		pressKey(boxes()[2], 'Backspace', 8);
		assert.strictEqual(document.activeElement, boxes()[1]);
		assert.deepStrictEqual(boxes().map(box => box.value), ['7', '', '', '', '', '']);
	});
	pairingTest('a pasted code fills every box and offers Approve', async () => {
		await approve();
		const pasted = new DataTransfer();
		pasted.setData('text/plain', ' 7-3-1-9-0-4 ');
		boxes()[1].dispatchEvent(new ClipboardEvent('paste', { clipboardData: pasted, bubbles: true, cancelable: true }));
		assert.strictEqual(boxes().map(box => box.value).join(''), '731904');
		assert.strictEqual(primary()?.classList.contains('disabled'), false);
	});
	pairingTest('approving sends the typed digits and shows the computer paired', async () => {
		await paired();
		assert.strictEqual(heading(), 'Computer paired');
		assert.strictEqual(fixture.confirmation, '731904');
	});
	pairingTest('the pairing\'s outcome is not counted as a step', async () => {
		await paired();
		assert.strictEqual(step(), '');
	});
	pairingTest('pressing outside closes the popover', async () => {
		await paired();
		document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
		assert.strictEqual(popover(), null);
	});
	pairingTest('opening an invitation after a pairing shows the chooser as the first of three steps', async () => {
		await paired();
		document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
		click(fixture.anchor('import'));
		assert.deepStrictEqual({ heading: heading(), step: step(), primary: primary()?.textContent }, { heading: 'Open an invitation', step: '1 of 3', primary: 'Choose invitation…' });
	});
	pairingTest('a dropped invitation asks for the creator\'s digits as the second of three steps', async () => {
		await dropInvitation();
		assert.strictEqual(boxes().length, 6);
		assert.deepStrictEqual({ file: text('.device-pairing-file-name'), code: text('.device-pairing-code'), step: step() }, { file: 'office-pc.vnl', code: '', step: '2 of 3' });
	});
	pairingTest('an incomplete code is refused as an error, not sent', async () => {
		await dropInvitation();
		typeCode(boxes(), '42');
		pressKey(boxes()[2], 'Enter', 13);
		await timeout(0);
		assert.strictEqual(fixture.confirmation, '731904');
		assert.strictEqual(text('.device-feedback'), 'Enter the six digits shown on the other computer.');
		assert.notStrictEqual(popover()?.querySelector('.device-feedback.error'), null);
	});
	pairingTest('a whole code pairs, then shows this computer\'s code while waiting for approval', async () => {
		await dropInvitation();
		typeCode(boxes(), '042517');
		pressKey(boxes()[5], 'Enter', 13);
		await timeout(0);
		assert.strictEqual(text('.device-pairing-code'), '918273');
		assert.strictEqual(fixture.confirmation, '042517');
		assert.strictEqual(step(), '3 of 3');
		assert.notStrictEqual(text('.device-pairing-waiting'), '');
	});
	pairingTest('a pairing button closes its own open popover', async () => {
		await dropInvitation();
		click(fixture.anchor('import'));
		assert.strictEqual(popover(), null);
	});
	pairingTest('disposing the page closes its popover', async () => {
		await invite();
		fixture.dispose();
		assert.strictEqual(popover(), null);
	});
});

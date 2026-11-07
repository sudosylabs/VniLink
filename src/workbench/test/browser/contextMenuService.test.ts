// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import '../../../platform/theme/browser/media/colors.css';
import assert from 'assert';
import { $ } from '../../../base/browser/dom.js';
import { animationFrames } from '../../../base/test/browser/waits.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../base/test/common/utils.js';
import { ContextMenuFixture } from './contextMenuFixture.js';

function press(key: string, keyCode: number): void {
	const target = document.activeElement ?? document.body;
	for (const type of ['keydown', 'keyup']) {
		target.dispatchEvent(new KeyboardEvent(type, { key, code: key, keyCode, bubbles: true, cancelable: true }));
	}
}

/** The colour `token` resolves to, as computed styles report it. */
function resolved(property: 'color' | 'backgroundColor', token: string): string {
	const probe = $('div');
	probe.style[property] = `var(${token})`;
	document.body.append(probe);
	const value = getComputedStyle(probe)[property];
	probe.remove();
	return value;
}

function menu(): HTMLElement | null {
	return document.querySelector<HTMLElement>('.context-view.monaco-menu-container .monaco-menu');
}

function items(): HTMLElement[] {
	return [...menu()?.querySelectorAll<HTMLElement>('.action-menu-item') ?? []];
}

/**
 * The context menu service, as the workbench registers it, opens a themed
 * menu with product icons, runs the chosen action, closes on Escape without
 * running anything, and breaks no CSP rule on the way.
 */
suite('ContextMenuService', () => {
	// First, so the fixture built in `setup` is tracked; its store is released before the check.
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: ContextMenuFixture;
	let violations: string[];
	const onViolation = (event: SecurityPolicyViolationEvent): void => {
		violations.push(`${event.violatedDirective} ${event.blockedURI}`);
	};
	setup(async () => {
		violations = [];
		document.addEventListener('securitypolicyviolation', onViolation);
		fixture = disposables.add(new ContextMenuFixture());
		fixture.show();
		await animationFrames(1);
	});
	teardown(async () => {
		document.removeEventListener('securitypolicyviolation', onViolation);
		if (menu()) {
			press('Escape', 27);
			await animationFrames(1);
		}
	});
	test('opens a menu with its two actions', () => {
		assert.deepStrictEqual(items().map(item => item.querySelector('.action-label')?.textContent), ['Rename', 'Show Hidden Files']);
	});
	test('announces an action as a menu item and a checked one as a checked menu item', () => {
		const [rename, hidden] = items();
		assert.strictEqual(rename.getAttribute('role'), 'menuitem');
		assert.strictEqual(hidden.getAttribute('role'), 'menuitemcheckbox');
		assert.strictEqual(hidden.getAttribute('aria-checked'), 'true');
	});
	test('draws the separator as a solid line in the line colour', () => {
		const separator = menu()?.querySelector<HTMLElement>('.action-label.separator');
		assert.ok(separator);
		assert.strictEqual(getComputedStyle(separator).borderBottomStyle, 'solid');
		assert.strictEqual(getComputedStyle(separator).borderBottomColor, resolved('color', '--line'));
	});
	test('is drawn on the raised surface', () => {
		const surface = menu()?.parentElement;
		assert.ok(surface);
		assert.notStrictEqual(resolved('backgroundColor', '--surface-raised'), 'rgba(0, 0, 0, 0)', 'the theme defines the raised surface');
		assert.strictEqual(getComputedStyle(surface).backgroundColor, resolved('backgroundColor', '--surface-raised'));
	});
	test('writes in the theme text colour', () => {
		assert.strictEqual(getComputedStyle(items()[1]).color, resolved('color', '--text'));
	});
	test('marks the checked item with a product icon, not a codicon', () => {
		const check = items()[1].querySelector('.menu-item-check svg');
		assert.ok(check);
		assert.ok(check.getBoundingClientRect().width > 0);
		assert.strictEqual(items()[0].querySelector('.codicon'), null);
	});
	test('focuses the first item', () => {
		assert.strictEqual(document.activeElement, items()[0]);
	});
	test('draws the focused item in the accent', () => {
		assert.strictEqual(getComputedStyle(items()[0]).backgroundColor, resolved('backgroundColor', '--accent'));
	});
	test('Enter runs the focused action once and closes the menu', async () => {
		press('Enter', 13);
		await animationFrames(1);
		assert.deepStrictEqual(fixture.runs, ['rename']);
		assert.strictEqual(document.querySelector('.monaco-menu'), null);
		assert.deepStrictEqual(fixture.hides, [false]);
	});
	test('Escape closes the menu as cancelled without running anything', async () => {
		press('Escape', 27);
		await animationFrames(1);
		assert.strictEqual(document.querySelector('.monaco-menu'), null);
		assert.deepStrictEqual(fixture.hides, [true]);
		assert.deepStrictEqual(fixture.runs, []);
	});
	test('opens again after an action ran', async () => {
		press('Enter', 13);
		await animationFrames(1);
		fixture.show();
		await animationFrames(1);
		assert.notStrictEqual(document.querySelector('.monaco-menu'), null);
	});
	test('breaks no content security rule', async () => {
		press('Enter', 13);
		await animationFrames(1);
		fixture.show();
		await animationFrames(1);
		press('Escape', 27);
		await animationFrames(1);
		assert.deepStrictEqual(violations, []);
	});
});

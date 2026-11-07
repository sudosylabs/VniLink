// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { Icons } from '../../../../../common/icons.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../test/common/utils.js';
import { renderIcon } from '../../icon.js';

suite('renderIcon', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	let host: HTMLElement;
	setup(() => {
		host = document.createElement('div');
		document.body.append(host);
	});
	teardown(() => {
		host.remove();
	});

	/** `id` drawn in both variants and placed on the page, so its styles apply. */
	function both(id: typeof Icons[keyof typeof Icons]): readonly [HTMLSpanElement, HTMLSpanElement] {
		const outlined = renderIcon(id);
		const filled = renderIcon(id, { variant: 'filled' });
		host.append(outlined, filled);
		return [outlined, filled];
	}
	for (const id of Object.values(Icons)) {
		suite(id, () => {
			test('draws artwork in both variants', () => {
				for (const icon of both(id)) {
					assert.notStrictEqual(icon.querySelector('svg path'), null);
				}
			});
			test('draws the filled variant differently from the outlined one', () => {
				const [outlined, filled] = both(id);
				assert.notStrictEqual(outlined.querySelector('svg')?.innerHTML, filled.querySelector('svg')?.innerHTML);
			});
		});
	}
	test('an icon is sixteen pixels wide in both variants', () => {
		for (const icon of both(Icons.home)) {
			assert.strictEqual(icon.querySelector('svg')?.getBoundingClientRect().width, 16);
		}
	});
	test('an icon is hidden from assistive technology and takes no focus', () => {
		for (const icon of both(Icons.home)) {
			assert.strictEqual(icon.querySelector('svg')?.getAttribute('focusable'), 'false');
			assert.strictEqual(icon.getAttribute('aria-hidden'), 'true');
		}
	});
	test('an icon fills with the colour of the text around it', () => {
		for (const icon of both(Icons.home)) {
			const svg = icon.querySelector('svg');
			assert.ok(svg);
			assert.strictEqual(getComputedStyle(svg).fill, getComputedStyle(icon).color);
		}
	});
	test('changing one rendered icon leaves the others intact', () => {
		const [outlined, filled] = both(Icons.home);
		outlined.querySelector('path')?.remove();
		assert.notStrictEqual(filled.querySelector('path'), null);
		assert.notStrictEqual(renderIcon(Icons.home).querySelector('path'), null);
	});
	test('a labelled icon is an image named by its label', () => {
		const named = renderIcon(Icons.home, { label: '<Home>' });
		assert.strictEqual(named.getAttribute('role'), 'img');
		assert.strictEqual(named.getAttribute('aria-label'), '<Home>');
		assert.strictEqual(named.hasAttribute('aria-hidden'), false);
	});
	test('a label is text, never markup', () => {
		const named = renderIcon(Icons.home, { label: '<Home>' });
		assert.strictEqual(named.querySelector('Home'), null);
	});
});

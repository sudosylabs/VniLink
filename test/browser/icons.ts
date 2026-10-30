// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Icons } from '../../src/base/common/icons.js';
import { renderIcon } from '../../src/base/browser/ui/icon/icon.js';
import { button } from '../../src/base/browser/ui/button/vnilinkButton.js';

function assert(value: boolean, message: string): void {
	if (!value) {
		throw new Error(message);
	}
}

export function verifyIcons(): boolean {
	const fixture = document.createElement('div');
	document.body.append(fixture);
	try {
		for (const id of Object.values(Icons)) {
			const outlined = renderIcon(id);
			const filled = renderIcon(id, { variant: 'filled' });
			fixture.append(outlined, filled);
			for (const icon of [outlined, filled]) {
				const svg = icon.querySelector('svg');
				assert(svg !== null && svg.querySelector('path') !== null, `Missing artwork: ${id}`);
				assert(svg?.getBoundingClientRect().width === 16, `Incorrect icon sizing: ${id}`);
				assert(svg?.getAttribute('focusable') === 'false' && icon.getAttribute('aria-hidden') === 'true', `Decorative icon exposed: ${id}`);
				assert(svg !== null && getComputedStyle(svg).fill === getComputedStyle(icon).color, `Icon does not inherit color: ${id}`);
			}
			assert(outlined.querySelector('svg')?.innerHTML !== filled.querySelector('svg')?.innerHTML, `Missing variant: ${id}`);
			outlined.querySelector('path')?.remove();
			assert(filled.querySelector('path') !== null && renderIcon(id).querySelector('path') !== null, `Shared DOM mutated: ${id}`);
		}
		const named = renderIcon(Icons.home, { label: '<Home>' });
		assert(named.getAttribute('role') === 'img' && named.getAttribute('aria-label') === '<Home>' && !named.hasAttribute('aria-hidden'), 'Named icon accessibility failed');
		assert(named.querySelector('Home') === null, 'Label treated as markup');
		let clicks = 0;
		const action = button('Back', 'test-button', {
			icon: Icons.back, onClick: () => {
				clicks++;
			}
		});
		action.click();
		assert(clicks === 1 && action.textContent === 'Back' && action.querySelector('[aria-hidden="true"]') !== null, 'Icon button behavior changed');
		return true;
	} finally {
		fixture.remove();
	}
}

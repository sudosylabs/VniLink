// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import '../../browser/media/colors.css';
import { $ } from '../../../../base/browser/dom.js';
import { Emitter } from '../../../../base/common/event.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { upcastPartial } from '../../../../base/test/common/mock.js';
import type { INativeHostService, IWindowState } from '../../../native/common/native.js';
import { ThemeService } from '../../browser/themeService.js';

const product = 'rgb(84, 126, 234)';

/** The palette colour `role` resolves to on the page, as `rgb(…)`. */
function resolved(role: string): string {
	const probe = document.body.appendChild($('span'));
	probe.style.color = `var(${role})`;
	const color = getComputedStyle(probe).color;
	probe.remove();
	return color;
}

suite('ThemeService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	const dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
	let states: Emitter<IWindowState>;

	/** A theme service in a window whose system accent is `accentColor`. */
	async function create(accentColor: string | undefined): Promise<void> {
		states = disposables.add(new Emitter<IWindowState>());
		const state: IWindowState = { windowId: 1, revision: 1, focused: true, maximized: false, fullscreen: false, alwaysOnTop: false, zoomFactor: 1, backgroundMaterial: 'none', accentColor };
		const nativeHostService = upcastPartial<INativeHostService>({ onDidChangeWindowState: states.event, getWindowState: () => Promise.resolve(state) });
		disposables.add(new ThemeService(nativeHostService));
		await Promise.resolve();
	}
	teardown(() => {
		document.documentElement.style.removeProperty('--system-accent');
		document.documentElement.style.removeProperty('--system-on-accent');
	});
	test('the accent is the system\'s, with white text on it', async () => {
		await create('#a550a7');
		assert.deepStrictEqual([resolved('--accent'), resolved('--focus-border'), resolved('--on-accent')], ['rgb(165, 80, 167)', 'rgb(165, 80, 167)', 'rgb(255, 255, 255)']);
	});
	test('without a system accent the product\'s stays', async () => {
		await create(undefined);
		assert.strictEqual(resolved('--accent'), product);
	});
	test('an accent that does not read on this theme\'s pages leaves the product\'s, and one that reads takes the text that reads on it', async () => {
		await create('#ffc600');
		// Yellow is lost on the light canvas; on the dark one it reads, and only dark text reads on it.
		assert.deepStrictEqual([resolved('--accent'), resolved('--on-accent')], dark ? ['rgb(255, 198, 0)', 'rgb(0, 0, 0)'] : [product, 'rgb(255, 255, 255)']);
	});
	test('a new system accent replaces the old one', async () => {
		await create('#a550a7');
		states.fire({ windowId: 1, revision: 2, focused: true, maximized: false, fullscreen: false, alwaysOnTop: false, zoomFactor: 1, backgroundMaterial: 'none', accentColor: '#0a84ff' });
		assert.strictEqual(resolved('--accent'), 'rgb(10, 132, 255)');
		states.fire({ windowId: 1, revision: 3, focused: true, maximized: false, fullscreen: false, alwaysOnTop: false, zoomFactor: 1, backgroundMaterial: 'none', accentColor: undefined });
		assert.strictEqual(resolved('--accent'), product);
	});
});

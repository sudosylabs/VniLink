// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { nativeTheme } from 'electron';
import Mocha from 'mocha';
import type { Automation } from '../../automation/src/automation.js';

/** A theme the native window can be put in. */
export type NativeTheme = 'system' | 'light' | 'dark';

/**
 * Puts the window's size and native theme back after the suite it is called
 * in, whatever its tests changed, so the next area starts where it expects.
 */
export function restoresWindow(app: Automation): void {
	let size: number[] = [];
	let theme: NativeTheme = 'system';
	Mocha.before(() => {
		size = app.window.getSize();
		theme = nativeTheme.themeSource;
	});
	Mocha.after(() => {
		app.window.setSize(size[0], size[1]);
		nativeTheme.themeSource = theme;
	});
}

/** Puts the native window in `theme` and waits for the page to draw it. */
export async function setNativeTheme(app: Automation, theme: NativeTheme): Promise<void> {
	nativeTheme.themeSource = theme;
	await app.driver.renderer.whenRendered();
}

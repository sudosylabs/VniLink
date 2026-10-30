// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { BrowserWindow } from 'electron';
import { isMacintosh } from '../../base/common/platform.js';
import { isBuilt } from '../../platform/environment/electron-main/environment.js';

/** Matches VS Code's development-only Reload Window and Toggle Developer Tools shortcuts. */
export function registerDevelopmentWindow(window: BrowserWindow): void {
	if (isBuilt() || process.env.VNILINK_DEV !== '1') {
		return;
	}
	window.webContents.on('before-input-event', (event, input) => {
		const modifier = isMacintosh ? input.meta : input.control;
		if (input.type !== 'keyDown' || !modifier) {
			return;
		}
		const key = input.key.toLowerCase();
		if (!input.alt && !input.shift && key === 'r') {
			event.preventDefault();
			window.webContents.reloadIgnoringCache();
		} else if ((isMacintosh ? input.alt && !input.shift : input.shift && !input.alt) && (key === 'i' || input.code === 'KeyI')) {
			event.preventDefault();
			window.webContents.toggleDevTools();
		}
	});
}

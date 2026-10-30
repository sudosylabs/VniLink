/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import type { BrowserWindow } from 'electron';
import type { WindowBackgroundMaterial } from '../../window/common/windowMaterial.js';
import { release } from 'node:os';
import { isMacintosh } from '../../../base/common/platform.js';
import { getMacOSWindowControlsPosition } from '../../window/common/window.js';
import type { IWindowControlsOptions } from '../../native/common/native.js';

export class CodeWindow {
	constructor(public readonly win: BrowserWindow, private readonly material: WindowBackgroundMaterial = 'none') { }

	public updateWindowControls(options: IWindowControlsOptions): void {
		if (this.win.isDestroyed()) {
			return;
		}
		if (isMacintosh) {
			if (options.height !== undefined) {
				this.win.setWindowButtonPosition(
					getMacOSWindowControlsPosition(options.height, release(), options.horizontalInset),
				);
			}
		} else {
			this.win.setTitleBarOverlay({
				color: this.material === 'mica' ? '#00000000' : options.backgroundColor,
				symbolColor: options.foregroundColor,
				height: options.height ? options.height - 1 : undefined,
			});
		}
	}
}

/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import type { BrowserWindow } from 'electron';
import { release } from 'node:os';
import { getMacOSWindowControlsPosition } from '../../window/common/window.js';
import type { IWindowControlsOptions } from '../../native/common/native.js';
export class CodeWindow {
	constructor(public readonly win: BrowserWindow) { }
	public updateWindowControls(options: IWindowControlsOptions): void {
		if (this.win.isDestroyed()) {
			return;
		}
		if (process.platform === 'darwin') {
			if (options.height !== undefined) {
				this.win.setWindowButtonPosition(
					getMacOSWindowControlsPosition(options.height, release(), options.horizontalInset),
				);
			}
		} else {
			this.win.setTitleBarOverlay({
				color: options.backgroundColor,
				symbolColor: options.foregroundColor,
				height: options.height ? options.height - 1 : undefined,
			});
		}
	}
}

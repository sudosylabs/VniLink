/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/workbench/services/clipboard/electron-browser/clipboardService.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { IClipboardService } from '../../../platform/clipboard/common/clipboardService.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { INativeHostService } from '../../../platform/native/common/native.js';

/**
 * The system clipboard through the main process, as VS Code's
 * `NativeClipboardService` (workbench/services/clipboard/electron-browser)
 * reaches it: the window's session refuses web clipboard permissions.
 */
export class NativeClipboardService implements IClipboardService {
	declare public readonly _serviceBrand: undefined;

	constructor(@INativeHostService private readonly nativeHostService: INativeHostService) { }

	public async writeText(text: string, type?: 'selection' | 'clipboard'): Promise<void> {
		return this.nativeHostService.writeClipboardText(text, type);
	}

	public async readText(type?: 'selection' | 'clipboard'): Promise<string> {
		return this.nativeHostService.readClipboardText(type);
	}
}

registerSingleton(IClipboardService, NativeClipboardService, InstantiationType.Delayed);

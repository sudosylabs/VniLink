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

import { Disposable } from '../../../base/common/lifecycle.js';
import { Emitter } from '../../../base/common/event.js';
import { INativeHostService } from '../../../platform/native/common/native.js';
import { IHostService } from './host.js';
import { registerSingleton, InstantiationType } from '../../../platform/instantiation/common/extensions.js';

export class WorkbenchHostService extends Disposable implements IHostService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<boolean>());
	public readonly onDidChangeFocus = this.changed.event;
	private focused = document.hasFocus();

	public get hasFocus(): boolean {
		return this.focused;
	}

	constructor(@INativeHostService nativeHostService: INativeHostService) {
		super();
		this._register(
			nativeHostService.onDidChangeWindowState((state) => {
				if (this.focused !== state.focused) {
					this.focused = state.focused;
					this.changed.fire(state.focused);
				}
			}),
		);
	}
}

registerSingleton(IHostService, WorkbenchHostService, InstantiationType.Delayed);

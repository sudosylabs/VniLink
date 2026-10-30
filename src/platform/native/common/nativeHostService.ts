/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { ProxyChannel } from '../../../base/parts/ipc/common/ipc.js';
import { IMainProcessService } from '../../ipc/common/mainProcessService.js';
import type { INativeHostService } from './native.js';

// @ts-expect-error: interface is implemented via proxy
export class NativeHostService implements INativeHostService {
	declare public readonly _serviceBrand: undefined;

	constructor(
		public readonly windowId: number,
		@IMainProcessService mainProcessService: IMainProcessService,
	) {
		return ProxyChannel.toService<INativeHostService>(mainProcessService.getChannel('nativeHost'), {
			context: windowId,
			properties: (() => {
				const properties = new Map<string, unknown>();
				properties.set('windowId', windowId);
				properties.set('dispose', undefined);
				return properties;
			})(),
		});
	}
}

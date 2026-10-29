/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { Event } from '../../../base/common/event.js';

export const IHostService = createDecorator<IHostService>('hostService');

export interface IHostService {
	readonly _serviceBrand: undefined;
	readonly onDidChangeFocus: Event<boolean>;
	readonly hasFocus: boolean;
}

/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import { createDecorator } from '../../instantiation/common/instantiation.js';
import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';

export const IMainProcessService = createDecorator<IMainProcessService>('mainProcessService');

export interface IMainProcessService {
	readonly _serviceBrand: undefined;
	getChannel(channelName: string): IChannel;
}

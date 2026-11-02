// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { InstantiationType, registerSingleton } from '../../instantiation/common/extensions.js';
import type { DataChannelRequest, DataPacket } from '../common/dataChannel.js';
import { IDataChannelService, type IDataChannel } from '../common/dataChannelService.js';
import { IEngineService } from '../common/engineService.js';
import { DataChannelClient } from './dataChannelClient.js';

/** Opens each channel as a {@link DataChannelClient} on the port main transfers to this window. */
export class DataChannelService implements IDataChannelService {
	declare public readonly _serviceBrand: undefined;

	constructor(@IEngineService private readonly engine: IEngineService) { }

	public open(
		request: Omit<DataChannelRequest, 'requestId'>,
		consume: (packet: DataPacket) => void | Promise<void>,
		onError: (error: Error) => void,
	): Promise<IDataChannel> {
		return DataChannelClient.open(this.engine, request, consume, onError);
	}
}

registerSingleton(IDataChannelService, DataChannelService, InstantiationType.Delayed);

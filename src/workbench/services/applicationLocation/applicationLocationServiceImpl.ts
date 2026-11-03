// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';
import { APPLICATION_LOCATION_CHANNEL_NAME, isApplicationLocation, type ApplicationLocation } from '../../../platform/applicationLocation/common/applicationLocation.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IMainProcessService } from '../../../platform/ipc/common/mainProcessService.js';
import { IApplicationLocationService } from './applicationLocationService.js';

class ApplicationLocationService implements IApplicationLocationService {
	declare public readonly _serviceBrand: undefined;
	private readonly channel: IChannel;

	constructor(@IMainProcessService mainProcessService: IMainProcessService) {
		this.channel = mainProcessService.getChannel(APPLICATION_LOCATION_CHANNEL_NAME);
	}

	public async getLocation(): Promise<ApplicationLocation> {
		const location = await this.channel.call<unknown>('getLocation');
		if (!isApplicationLocation(location)) {
			throw new Error('Invalid application location');
		}
		return location;
	}

	public async move(): Promise<boolean> {
		return await this.channel.call<unknown>('move') === true;
	}
}

registerSingleton(IApplicationLocationService, ApplicationLocationService, InstantiationType.Delayed);

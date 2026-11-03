// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IChannel } from '../../../../base/parts/ipc/common/ipc.js';
import type { IMainProcessService } from '../../common/mainProcessService.js';

/** An {@link IMainProcessService} serving the channels a test gives it, by name; any other channel is refused. */
export class TestMainProcessService implements IMainProcessService {
	declare public readonly _serviceBrand: undefined;

	constructor(private readonly channels: Readonly<Record<string, IChannel>>) { }

	public getChannel(channelName: string): IChannel {
		const channel = this.channels[channelName] as IChannel | undefined;
		if (!channel) {
			throw new Error(`Unexpected channel ${channelName}`);
		}
		return channel;
	}
}

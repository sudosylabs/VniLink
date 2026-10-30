// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import { IConnectionService, type RunningRelays } from './connectionService.js';

export class ConnectionService implements IConnectionService {
	declare public readonly _serviceBrand: undefined;

	constructor(@IEngineService private readonly engine: IEngineService) { }

	public async getRunningRelays(): Promise<RunningRelays> {
		const reply = await this.engine.execute({ type: 'snapshot' });
		if (reply.type !== 'snapshot') {
			throw new Error('Invalid connection snapshot reply');
		}
		return Object.freeze({ publicRelay: reply.snapshot.public_relay, urls: Object.freeze([...reply.snapshot.relay_urls]) });
	}
}

registerSingleton(IConnectionService, ConnectionService, InstantiationType.Delayed);

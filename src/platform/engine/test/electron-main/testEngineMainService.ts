// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { MessagePortMain } from 'electron';
import type { DataChannelRequest } from '../../common/dataChannel.js';
import { TestEngineService } from '../common/testEngineService.js';

/**
 * The engine as main's services and channels use it: a {@link TestEngineService}
 * that also records the data channels attached to it and the owners whose
 * channels were closed.
 */
export class TestEngineMainService extends TestEngineService {
	public readonly isRestarting = false;
	public readonly attached: { readonly request: DataChannelRequest; readonly owner: string }[] = [];
	public readonly closedOwners: string[] = [];

	public getRestartRequired(): Promise<boolean> {
		return Promise.resolve(this.restartRequired);
	}

	public refreshRestartRequired(): Promise<void> {
		return Promise.resolve();
	}

	public attachChannel(_port: MessagePortMain, request: DataChannelRequest, owner: string): void {
		this.attached.push({ request, owner });
	}

	public closeChannels(owner: string): void {
		this.closedOwners.push(owner);
	}
}

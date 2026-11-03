// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Disposable } from '../../../base/common/lifecycle.js';
import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';
import { INVITATIONS_CHANNEL_NAME, isOpenedInvitations } from '../../../platform/invitations/common/invitations.js';
import { IMainProcessService } from '../../../platform/ipc/common/mainProcessService.js';
import { ILogService } from '../../../platform/log/common/log.js';
import type { IWorkbenchContribution } from '../../contributions.js';
import { IDevicePairingService } from '../../services/devices/devicePairingService.js';
import { IHomeService } from '../../services/home/homeService.js';

/**
 * Opens on the Devices page the invitations the operating system opened with
 * VniLink: the main process reads them and focuses the window, this takes
 * them. Several opened at once leave the last one showing, as choosing files
 * one after another in the app does.
 */
export class OpenedInvitationsContribution extends Disposable implements IWorkbenchContribution {
	public static readonly ID = 'workbench.contrib.openedInvitations';
	private readonly channel: IChannel;

	constructor(
		@IMainProcessService mainProcessService: IMainProcessService,
		@IHomeService private readonly homeService: IHomeService,
		@IDevicePairingService private readonly pairing: IDevicePairingService,
		@ILogService private readonly logService: ILogService,
	) {
		super();
		this.channel = mainProcessService.getChannel(INVITATIONS_CHANNEL_NAME);
		// Listening first: an invitation opened between the two is announced, not lost.
		this._register(this.channel.listen<unknown>('onDidOpenInvitations')(() => {
			void this.take();
		}));
		void this.take();
	}

	private async take(): Promise<void> {
		try {
			const invitations = await this.channel.call<unknown>('take');
			if (!isOpenedInvitations(invitations)) {
				throw new Error('Invalid opened invitations');
			}
			const last = invitations.at(-1);
			if (!last || this._store.isDisposed) {
				return;
			}
			// The page first: it shows the invitation once the pairing flow holds it.
			this.homeService.showPage('devices');
			await this.pairing.openInvitation(new File([new Uint8Array(last.bytes)], last.name));
		} catch (error) {
			this.logService.error(error instanceof Error ? error : new Error(String(error)));
		}
	}
}

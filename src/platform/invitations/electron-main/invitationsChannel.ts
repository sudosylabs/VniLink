// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Event } from '../../../base/common/event.js';
import type { IServerChannel } from '../../../base/parts/ipc/common/ipc.js';
import { windowIdFromContext } from '../../ipc/electron-main/windowContext.js';
import type { InvitationsMainService } from './invitationsMainService.js';

/** Tells windows that invitations were opened, and hands them out once each. */
export class InvitationsChannel implements IServerChannel {

	constructor(private readonly invitations: InvitationsMainService) { }

	public listen<T>(_context: string, event: string): Event<T> {
		if (event === 'onDidOpenInvitations') {
			return this.invitations.onDidOpenInvitations as Event<T>;
		}
		throw new Error(`Unknown invitations event: ${event}`);
	}

	public call<T>(context: string, command: string): Promise<T> {
		windowIdFromContext(context);
		if (command !== 'take') {
			throw new Error(`Unknown invitations command: ${command}`);
		}
		return Promise.resolve(this.invitations.take() as T);
	}
}

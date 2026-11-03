// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Event } from '../../../base/common/event.js';
import type { IServerChannel } from '../../../base/parts/ipc/common/ipc.js';
import { windowIdFromContext } from '../../ipc/electron-main/windowContext.js';
import type { ApplicationLocationMainService } from './applicationLocationMainService.js';

/** Tells windows whether the application can move into the Applications folder, and moves it. */
export class ApplicationLocationChannel implements IServerChannel {

	constructor(private readonly location: ApplicationLocationMainService) { }

	public listen<T>(_context: string, event: string): Event<T> {
		throw new Error(`Unknown application location event: ${event}`);
	}

	public call<T>(context: string, command: string): Promise<T> {
		windowIdFromContext(context);
		switch (command) {
			case 'getLocation':
				return Promise.resolve(this.location.location as T);
			case 'move':
				return Promise.resolve(this.location.move() as T);
		}
		throw new Error(`Unknown application location command: ${command}`);
	}
}

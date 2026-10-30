// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IEngineService } from '../../../platform/engine/common/engineService.js';

/** The connection a service on a paired computer opens over. */
export interface ControllerSession {
	readonly id: string;
	readonly generation: string;
}

/** This computer's active connection to `deviceId`, if it has one. */
export async function activeControllerSession(engine: IEngineService, deviceId: string): Promise<ControllerSession | undefined> {
	if (!engine.available) {
		return undefined;
	}
	try {
		const reply = await engine.execute({ type: 'controllerSessions' });
		if (reply.type !== 'sessions') {
			return undefined;
		}
		const session = reply.sessions.find(item => item.role === 'Controller' && item.peer_id === deviceId && item.phase === 'Active');
		return session && { id: session.id, generation: session.generation };
	} catch {
		return undefined;
	}
}

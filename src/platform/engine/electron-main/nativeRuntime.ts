// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { PermissionSnapshot } from '../../permissions/common/permissions.js';
import type { EngineMainService } from './engineMainService.js';

/** No permission prompts, capture sessions or shell processes are started here. */
export async function synchronizeNativePermissions(engine: Pick<EngineMainService, 'available' | 'execute'>, platform: string, snapshot: PermissionSnapshot): Promise<void> {
	if (!engine.available) {
		return;
	}
	if (platform === 'darwin') {
		const allowed = (kind: string): boolean => snapshot.permissions.some(permission => permission.kind === kind && permission.status === 'granted');
		await engine.execute({ type: 'reportNativePermissions', screen: allowed('screen'), input: allowed('accessibility'), camera: allowed('camera'), microphone: allowed('microphone') });
		if (allowed('accessibility')) {
			await engine.execute({ type: 'enableNativeInput' });
		}
	} else if (platform === 'win32' || platform === 'linux') {
		await engine.execute({ type: 'enableNativeInput' });
	}
}

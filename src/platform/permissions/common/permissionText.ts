// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { isMacintosh } from '../../../base/common/platform.js';
import type { MachinePermission, PermissionKind, PermissionStatus } from './permissions.js';

/** Each permission by the name the system's own settings give it. */
export const permissionTitles: Readonly<Record<PermissionKind, string>> = {
	screen: nls.localize('permissions.screen', 'Screen Recording'),
	accessibility: nls.localize('permissions.accessibility', 'Accessibility'),
	camera: nls.localize('permissions.camera', 'Camera'),
	microphone: nls.localize('permissions.microphone', 'Microphone'),
};

/** Only where the system's own name does not say what VniLink uses it for. */
export const permissionDescriptions: Readonly<Partial<Record<PermissionKind, string>>> = {
	accessibility: nls.localize('permissions.accessibilityHelp', 'Needed to control this computer’s keyboard and mouse.'),
};

export const permissionStatuses: Readonly<Record<PermissionStatus, string>> = {
	granted: nls.localize('permissions.granted', 'Allowed'),
	denied: nls.localize('permissions.denied', 'Not allowed'),
	restricted: nls.localize('permissions.restricted', 'Restricted'),
	'not-determined': nls.localize('permissions.notDetermined', 'Not requested'),
	unknown: nls.localize('permissions.unknown', 'Status unavailable'),
	unsupported: nls.localize('permissions.unsupported', 'Not used on this system'),
	'not-required': nls.localize('permissions.notRequired', 'No permission needed'),
};

/** The action a permission offers: asking the system, or opening its settings. */
export function permissionActionLabel(permission: MachinePermission): string {
	return permission.canRequest ? nls.localize('permissions.request', 'Request access')
		: isMacintosh ? nls.localize('permissions.systemSettings', 'Open System Settings') : nls.localize('permissions.settings', 'Open Settings');
}

/** Whether the permission has an action left: allowed access needs none here, though the system can still revoke it. */
export function permissionNeedsAction(permission: MachinePermission): boolean {
	return permission.status !== 'granted' && (permission.canRequest || permission.canOpenSettings);
}

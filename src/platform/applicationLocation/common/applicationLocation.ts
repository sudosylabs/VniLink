// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Main-process IPC channel saying whether the application can move into the Applications folder, and moving it. */
export const APPLICATION_LOCATION_CHANNEL_NAME = 'applicationLocation';

/**
 * Where the running application is. `movable`: a built macOS application
 * outside the Applications folders, such as one opened from the disk image
 * or from Downloads, which should move there to keep its permissions.
 */
export interface ApplicationLocation {
	readonly movable: boolean;
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { getNativeTitlebarHeight } from './window.js';

/** Room above the fixed-height view tabs, shared with the native window controls. */
export function getWorkbenchTitlebarHeight(osVersion: string): number {
	return Math.max(40, getNativeTitlebarHeight(osVersion));
}

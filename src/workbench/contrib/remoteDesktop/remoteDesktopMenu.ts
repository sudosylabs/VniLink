// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { MenuId } from '../../../platform/actions/common/actions.js';

/**
 * Commands on the computer whose screen a remote desktop shows, in its
 * session bar's More menu after the tab's own. Each runs with that
 * computer's device id.
 */
export const RemoteDesktopMenu = new MenuId('RemoteDesktop');

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Entry of the restart notice window; see `platform/systemActions/browser/noticePage.ts`. */
import { mainWindow } from './base/browser/window.js';
import { NoticePage, type NoticeBridge } from './platform/systemActions/browser/noticePage.js';

const bridge: unknown = Reflect.get(mainWindow, 'vnilinkNotice');

if (typeof bridge === 'object' && bridge !== null) {
	new NoticePage(mainWindow.document.body, bridge as NoticeBridge);
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Whether this process runs an installed build rather than the source
 * checkout, as VS Code's `isBuilt`. A checkout is launched as
 * `electron <path>`, which Electron reports as `process.defaultApp`. Unlike
 * `app.isPackaged`, this does not depend on the executable's name, which the
 * development Electron (`build/lib/electron.ts`) shares with the product.
 */
export function isBuilt(): boolean {
	return !process.defaultApp;
}

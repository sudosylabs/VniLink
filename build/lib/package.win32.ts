// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The Windows installer: an assisted, per-user NSIS setup without elevation.
 * It welcomes the person with VniLink's artwork, shows the license as
 * information, lets them choose the folder and a desktop shortcut
 * (`windows/installer.nsh`), and offers to start VniLink when it finishes.
 */
import { join } from 'node:path';
import type { BuilderConfiguration } from './package.ts';
import type { Product } from './product.ts';

export const extensions = ['.exe'];

const sidebar = 'assets/windows/installer-sidebar.bmp';

const header = 'assets/windows/installer-header.bmp';

const include = 'build/lib/windows/installer.nsh';

/** Files the release stages into `packaging-resources` for this installer, at the same paths. */
export const resources: readonly string[] = [sidebar, header, include, 'LICENSE'];

export function installerOptions(product: Product, base: BuilderConfiguration): Record<string, unknown> {
	const staged = (path: string) => join(base.directories.buildResources, path);
	return {
		win: { ...base.win, target: ['nsis'] },
		nsis: {
			oneClick: false,
			perMachine: false,
			allowElevation: false,
			allowToChangeInstallationDirectory: true,
			packElevateHelper: false,
			createDesktopShortcut: false,
			createStartMenuShortcut: true,
			shortcutName: product.nameLong,
			installerIcon: base.win.icon,
			uninstallerIcon: base.win.icon,
			installerSidebar: staged(sidebar),
			uninstallerSidebar: staged(sidebar),
			installerHeader: staged(header),
			license: staged('LICENSE'),
			include: staged(include),
			deleteAppDataOnUninstall: false,
			runAfterFinish: true,
			differentialPackage: false,
		},
	};
}

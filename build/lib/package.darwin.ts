// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The macOS installer: a UDZO disk image whose window shows the application
 * beside an Applications link, over artwork pointing from one to the other
 * (`installerAssets.ts` renders it at 1x and 2x, which the packager combines).
 */
import { join } from 'node:path';
import { dmgLayout } from './installerAssets.ts';
import type { BuilderConfiguration } from './package.ts';
import type { Product } from './product.ts';

export const extensions = ['.dmg'];

/** Files the release stages into `packaging-resources` for this installer, at the same paths. */
export const resources: readonly string[] = ['assets/apple/dmg/background.png', 'assets/apple/dmg/background@2x.png'];

export function installerOptions(product: Product, base: BuilderConfiguration): Record<string, unknown> {
	const { width, height, application, applications } = dmgLayout;
	return {
		mac: { ...base.mac, target: ['dmg'] },
		dmg: {
			title: product.nameLong,
			format: 'UDZO',
			sign: false,
			writeUpdateInfo: false,
			background: join(base.directories.buildResources, resources[0]),
			iconSize: 96,
			window: { width, height },
			contents: [{ ...application, type: 'file' }, { ...applications, type: 'link', path: '/Applications' }],
		},
	};
}

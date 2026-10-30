// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** The macOS installer: a UDZO disk image with the application and an Applications link. */
import type { BuilderConfiguration } from './package.ts';
import type { Product } from './product.ts';

export const extensions = ['.dmg'];

export function installerOptions(product: Product, base: BuilderConfiguration): Record<string, unknown> {
	return {
		mac: { ...base.mac, target: ['dmg'] },
		dmg: {
			title: product.nameLong,
			format: 'UDZO',
			sign: false,
			writeUpdateInfo: false,
			backgroundColor: '#f5f5f5',
			window: { width: 540, height: 380 },
			contents: [{ x: 140, y: 180, type: 'file' }, { x: 400, y: 180, type: 'link', path: '/Applications' }],
		},
	};
}

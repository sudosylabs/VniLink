// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** The Linux installers: Debian and RPM packages with a desktop entry matching the application id. */
import assert from 'node:assert/strict';
import type { BuilderConfiguration } from './package.ts';
import type { Product } from './product.ts';

export const extensions = ['.deb', '.rpm'];

export function installerOptions(product: Product, base: BuilderConfiguration): Record<string, unknown> {
	return {
		linux: {
			...base.linux,
			target: ['deb', 'rpm'],
			maintainer: `${product.publisherName} <${product.supportEmail}>`,
			vendor: product.publisherName,
			synopsis: 'Connect your computers with VniLink',
			description: product.description,
			syncDesktopName: true,
			desktop: { entry: { Name: product.nameLong, StartupWMClass: product.platforms.linux.desktopFileName.replace(/\.desktop$/, '') } },
		},
	};
}

/** The keys of a desktop entry's `[Desktop Entry]` group. */
export function desktopEntryKeys(contents: string): ReadonlyMap<string, string> {
	const keys = new Map<string, string>();
	let group = '';
	for (const line of contents.split('\n')) {
		const header = /^\[(.+)\]$/.exec(line);
		if (header) {
			group = header[1];
		} else if (group === 'Desktop Entry' && line.includes('=') && !line.startsWith('#')) {
			keys.set(line.slice(0, line.indexOf('=')), line.slice(line.indexOf('=') + 1));
		}
	}
	return keys;
}

/**
 * What a session matches the installed application by: the entry named after
 * its app id, naming that id as its window class, and an icon at the sizes
 * docks and launchers ask for. `files` are the package's paths without a
 * leading slash; `entry` is the desktop entry it installs.
 */
export function checkDesktopIntegration(product: Product, files: readonly string[], entry: string): void {
	const desktopFile = `usr/share/applications/${product.platforms.linux.desktopFileName}`;
	assert(files.includes(desktopFile), `The package installs no ${desktopFile}`);
	const keys = desktopEntryKeys(entry);
	assert.equal(keys.get('StartupWMClass'), product.appId, 'The desktop entry names another window class than the app id');
	assert((keys.get('MimeType') ?? '').split(';').includes(product.invitation.mimeType), 'The desktop entry does not open invitations');
	const icon = keys.get('Icon');
	assert(icon, 'The desktop entry names no icon');
	for (const size of ['48x48', '256x256']) {
		assert(files.includes(`usr/share/icons/hicolor/${size}/apps/${icon}.png`), `The package installs no ${size} icon named ${icon}`);
	}
}

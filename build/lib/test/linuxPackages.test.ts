// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { checkDesktopIntegration, desktopEntryKeys } from '../package.linux.ts';
import { readProduct } from '../product.ts';

const product = await readProduct(fileURLToPath(new URL('../../../', import.meta.url)));

const entry = ['[Desktop Entry]', 'Name=VniLink', 'Icon=vnilink', `StartupWMClass=${product.appId}`, '', '[Desktop Action New]', 'Icon=other', ''].join('\n');

const desktopFile = `usr/share/applications/${product.platforms.linux.desktopFileName}`;

const files = [desktopFile, 'usr/share/icons/hicolor/48x48/apps/vnilink.png', 'usr/share/icons/hicolor/256x256/apps/vnilink.png'];

/** The failure `checkDesktopIntegration` reports for `packaged` files and `desktopEntry`. */
function refusal(packaged: readonly string[], desktopEntry: string): string {
	try {
		checkDesktopIntegration(product, packaged, desktopEntry);
	} catch (error) {
		return (error as Error).message;
	}
	assert.fail('The package was accepted');
}

suite('Linux package desktop integration', () => {
	test('reads only the Desktop Entry group', () => {
		assert.strictEqual(desktopEntryKeys(entry).get('Icon'), 'vnilink');
	});
	test('accepts the entry named after the app id with its window class and icon sizes', () => {
		checkDesktopIntegration(product, files, entry);
	});
	test('refuses a package without the entry named after the app id', () => {
		assert.strictEqual(refusal(['usr/share/applications/vnilink.desktop', ...files.slice(1)], entry), `The package installs no ${desktopFile}`);
	});
	test('refuses an entry whose window class is not the app id', () => {
		assert.match(refusal(files, entry.replace(`StartupWMClass=${product.appId}`, 'StartupWMClass=VniLink')), /window class/);
	});
	test('refuses a package with only a large icon', () => {
		assert.match(refusal([files[0], 'usr/share/icons/hicolor/1024x1024/apps/vnilink.png'], entry), /48x48 icon/);
	});
});

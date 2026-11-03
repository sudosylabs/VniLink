// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { dataHome, developmentDesktopEntry, developmentDesktopPaths, writeDevelopmentDesktopEntry } from '../linuxDesktop.ts';
import { readProduct } from '../product.ts';

const root = fileURLToPath(new URL('../../../', import.meta.url));

suite('linuxDesktop', () => {
	test('names the id the source checkout runs as, for its window class and icon', async () => {
		const product = await readProduct(root);
		const lines = developmentDesktopEntry(product, '/work/vnilink').split('\n');
		assert.ok(lines.includes(`StartupWMClass=${product.appId}.dev`), lines.join('\n'));
		assert.ok(lines.includes(`Icon=${product.appId}.dev`), lines.join('\n'));
		assert.ok(lines.includes('MimeType=application/x-vnilink-invitation;'), lines.join('\n'));
	});
	test('starts the development Electron with the checkout, quoting both', async () => {
		const lines = developmentDesktopEntry(await readProduct(root), '/work/Vni "Link"').split('\n');
		assert.ok(lines.includes(String.raw`Exec="/work/Vni \\"Link\\"/.build/electron/vnilink" "/work/Vni \\"Link\\"" %U`), lines.join('\n'));
	});
	test('refuses a checkout path with a line break', async () => {
		const product = await readProduct(root);
		assert.throws(() => developmentDesktopEntry(product, '/work/vni\nlink'), /cannot start/);
	});
	test('installs into the XDG data directory, or ~/.local/share without one', () => {
		assert.strictEqual(dataHome({ XDG_DATA_HOME: '/data' }), '/data');
		assert.match(dataHome({ XDG_DATA_HOME: '' }), /\.local\/share$/);
	});
	test('writes the entry and its scalable icon, and removes both', async () => {
		const home = await mkdtemp(join(tmpdir(), 'vnilink-desktop-'));
		try {
			const product = await readProduct(root);
			const paths = developmentDesktopPaths(product, home);
			assert.strictEqual(await writeDevelopmentDesktopEntry(root, home, false, AbortSignal.timeout(10_000)), paths.entry);
			assert.strictEqual(await readFile(paths.entry, 'utf8'), developmentDesktopEntry(product, root));
			assert.deepStrictEqual(await readFile(paths.icon), await readFile(join(root, 'assets/linux/app-icon.svg')));
			await writeDevelopmentDesktopEntry(root, home, true, AbortSignal.timeout(10_000));
			await assert.rejects(stat(paths.entry), { code: 'ENOENT' });
			await assert.rejects(stat(paths.icon), { code: 'ENOENT' });
		} finally {
			await rm(home, { recursive: true, force: true });
		}
	});
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { chmod, mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { validateConfiguration } from 'app-builder-lib/out/util/config/config.js';
import { buildInstallers, installerConfiguration, installerExtensions } from '../../build/lib/installers.ts';
import { packagedLayout } from '../../build/lib/package.ts';
import { fileSha256, packageInventory, releaseInventory, verifyInstallerFiles, verifyPackageFiles } from '../../build/lib/verify.ts';

const product = JSON.parse(await readFile(new URL('../../product.json', import.meta.url), 'utf8'));

test('native installer targets validate against the pinned builder schema and preserve user data', async () => {
	for (const platform of ['darwin', 'win32', 'linux']) {
		const config = installerConfiguration('/repo', '/stage', '/output', product, { platform, electronVersion: '44.7.0' });
		await validateConfiguration(config, { isEnabled: false });
		assert.equal(config.publish, null);
		assert.equal(config.win.signExecutable, false);
		if (platform === 'darwin') {
			assert.deepEqual(config.mac.target, ['dmg']);
			assert.equal(config.dmg.sign, false);
		} else if (platform === 'win32') {
			assert.deepEqual(config.win.target, ['nsis']);
			assert.equal(config.nsis.perMachine, false);
			assert.equal(config.nsis.deleteAppDataOnUninstall, false);
			assert.equal(config.nsis.runAfterFinish, false);
		} else {
			assert.deepEqual(config.linux.target, ['deb', 'rpm']);
			assert.equal(config.linux.syncDesktopName, true);
		}
		assert.equal(config.nsis === undefined, platform !== 'win32', 'Installer options leaked into another platform');
	}
	assert.throws(() => installerExtensions('unknown'), /Unsupported/);
});

async function fixture() {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-installer-contract-'));
	const target = `${process.platform}-${process.arch}`;
	const application = join(root, 'dist', target);
	const layout = packagedLayout(application, process.platform, product);
	await mkdir(join(layout.resources, 'app.asar.unpacked/native'), { recursive: true });
	await writeFile(layout.asar, 'test application archive');
	await writeFile(layout.native, 'test native library');
	const manifest = { appId: product.appId, version: '0.1.0', electronVersion: '44.7.0', platform: process.platform, architecture: process.arch, signing: 'unsigned', asarSha256: await fileSha256(layout.asar), nativeSha256: await fileSha256(layout.native), files: await packageInventory(application) };
	await writeFile(join(application, 'packaging-manifest.json'), JSON.stringify(manifest));
	const stage = join(root, '.build/release', target);
	await mkdir(join(stage, 'app'), { recursive: true });
	await writeFile(join(stage, 'app/product.json'), JSON.stringify(product));
	await writeFile(join(stage, 'build-manifest.json'), JSON.stringify({ profile: 'release', version: manifest.version, electronVersion: manifest.electronVersion, platform: process.platform, architecture: process.arch, files: await releaseInventory(stage) }));
	await writeFile(join(root, 'product.json'), JSON.stringify(product));
	return { root, target, application, layout, manifest, output: join(root, 'dist/installers', target) };
}

test('complete bundle inventory rejects added, modified and omitted resources', async () => {
	const f = await fixture();
	try {
		const verifier = { verifyFiles: () => verifyPackageFiles(f.application, product) };
		await verifier.verifyFiles();
		const extra = join(f.layout.resources, 'unexpected');
		await writeFile(extra, 'extra');
		await assert.rejects(verifier.verifyFiles(), /inventory differs/);
		await rm(extra);
		await writeFile(f.layout.native, 'changed native library');
		await assert.rejects(verifier.verifyFiles(), /inventory differs/);
		await rm(f.layout.native);
		await assert.rejects(verifier.verifyFiles(), /inventory differs/);
	} finally {
		await rm(f.root, { recursive: true, force: true });
	}
});

test('bundle inventory preserves framework links, rejects external links and detects executable permission changes', { skip: process.platform === 'win32' }, async () => {
	const f = await fixture();
	try {
		await symlink('app.asar', join(f.layout.resources, 'relative-link'));
		assert((await packageInventory(f.application)).some(file => file.type === 'link' && file.target === 'app.asar'));
		await symlink('/etc/passwd', join(f.layout.resources, 'external-link'));
		await assert.rejects(packageInventory(f.application), /escapes/);
		await rm(join(f.layout.resources, 'external-link'));
		const before = await packageInventory(f.application);
		await chmod(f.layout.native, 0o755);
		assert.notDeepEqual(await packageInventory(f.application), before);
	} finally {
		await rm(f.root, { recursive: true, force: true });
	}
});

test('installer publication preserves previous output on failure and rejects checksum corruption', async () => {
	const f = await fixture();
	try {
		await mkdir(f.output, { recursive: true });
		await writeFile(join(f.output, 'previous'), 'completed output');
		const signal = new AbortController().signal;
		await assert.rejects(buildInstallers(f.root, signal, {
			runBuilder: async () => {
				throw new Error('installer tool failed');
			},
		}), /installer tool failed/);
		assert.equal(await readFile(join(f.output, 'previous'), 'utf8'), 'completed output');
		assert(!(await readdir(join(f.root, 'dist/installers'))).some(name => name.startsWith('.installer-')));
		await assert.rejects(readFile(`${f.application}.lock`), /ENOENT/);
		await buildInstallers(f.root, signal, {
			runBuilder: async (_processes, args) => {
				assert(args.includes('--prepackaged'));
				assert.equal(args.at(-1), 'never');
				const config = JSON.parse(await readFile(args[args.indexOf('--config') + 1], 'utf8'));
				await mkdir(config.directories.output);
				for (const ext of installerExtensions(process.platform)) {
					await writeFile(join(config.directories.output, `VniLink-test${ext}`), 'installer artifact');
				}
			},
		});
		const manifest = await verifyInstallerFiles(f.output, product);
		await writeFile(join(f.output, manifest.artifacts[0].path), 'corrupt artifact');
		await assert.rejects(verifyInstallerFiles(f.output, product));
	} finally {
		await rm(f.root, { recursive: true, force: true });
	}
});

test('the builder packages a copy, so what it adds there leaves the verified application unchanged', async () => {
	const f = await fixture();
	try {
		const before = await packageInventory(f.application);
		await buildInstallers(f.root, new AbortController().signal, {
			runBuilder: async (_processes, args) => {
				// As electron-builder's deb target adds the sandbox's AppArmor profile to the application it packages.
				const prepackaged = args[args.indexOf('--prepackaged') + 1];
				assert(!prepackaged.startsWith(f.application), 'The builder was given the verified application itself');
				await writeFile(join(prepackaged, 'added-by-builder'), 'builder resource');
				const config = JSON.parse(await readFile(args[args.indexOf('--config') + 1], 'utf8'));
				await mkdir(config.directories.output);
				for (const ext of installerExtensions(process.platform)) {
					await writeFile(join(config.directories.output, `VniLink-test${ext}`), 'installer artifact');
				}
			},
		});
		assert.deepEqual(await packageInventory(f.application), before, 'The verified application changed');
	} finally {
		await rm(f.root, { recursive: true, force: true });
	}
});

test('installer generation respects an active assembly lock and cancellation prevents publication', async () => {
	const f = await fixture();
	try {
		await mkdir(`${f.application}.lock`);
		let calls = 0;
		await assert.rejects(buildInstallers(f.root, new AbortController().signal, {
			runBuilder: async () => {
				calls++;
			},
		}), /EEXIST/);
		assert.equal(calls, 0);
		await rm(`${f.application}.lock`, { recursive: true });
		await mkdir(f.output, { recursive: true });
		await writeFile(join(f.output, 'previous'), 'completed');
		const controller = new AbortController();
		await assert.rejects(buildInstallers(f.root, controller.signal, {
			runBuilder: async (_processes, args) => {
				const config = JSON.parse(await readFile(args[args.indexOf('--config') + 1], 'utf8'));
				await mkdir(config.directories.output);
				for (const ext of installerExtensions(process.platform)) {
					await writeFile(join(config.directories.output, `cancelled${ext}`), 'not published');
				}
				controller.abort(new Error('Installer generation cancelled'));
			},
		}), /cancelled/);
		assert.equal(await readFile(join(f.output, 'previous'), 'utf8'), 'completed');
		assert.deepEqual(await readdir(join(f.root, 'dist/installers')), [f.target]);
	} finally {
		await rm(f.root, { recursive: true, force: true });
	}
});

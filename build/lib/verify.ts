// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** The `verify:*` tasks: each checks a built artifact against its manifest, then runs it. */
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, relative } from 'node:path';
import { cp, mkdir, mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { verifyInstallerFiles } from './installers.ts';
import { packageInventory, verifyReleaseFiles } from './inventory.ts';
import { checkDesktopIntegration } from './package.linux.ts';
import { runPackagedApplication } from './packagedApplication.ts';
import { ProcessRunner } from './processes.ts';
import { readProduct, type Product } from './product.ts';
import { isLinux, isMacintosh } from './platform.ts';

interface TemporaryEngine {
	execute(command: string): Promise<string>;
}

/** How long the staged addon may take for each step of the release probe. */
const releaseProbeTimeout = 60_000;

/** `promise`, or a failure naming `what` if it has not settled in time. */
async function within<T>(promise: Promise<T>, what: string): Promise<T> {
	let timer: NodeJS.Timeout | undefined;
	const timeout = new Promise<never>((_resolve, reject) => {
		timer = setTimeout(() => {
			reject(new Error(`The staged addon did not finish ${what} in ${String(releaseProbeTimeout / 1000)} seconds`));
		}, releaseProbeTimeout);
	});
	try {
		return await Promise.race([promise, timeout]);
	} finally {
		clearTimeout(timer);
	}
}

/** Checks the staged payload, then loads its addon with an isolated temporary identity. */
export async function verifyRelease(root: string): Promise<void> {
	const directory = join(root, '.build/release', `${process.platform}-${process.arch}`);
	await verifyReleaseFiles(directory);
	const addon = createRequire(import.meta.url)(join(directory, 'app/native/vnilink.node')) as { openTemporaryEngine(): Promise<TemporaryEngine> };
	const engine = await within(addon.openTemporaryEngine(), 'opening a temporary engine');
	try {
		const reply = JSON.parse(await within(engine.execute(JSON.stringify({ type: 'snapshot' })), 'a snapshot')) as { type: string; snapshot: { endpoint_id: string; devices: unknown[] } };
		assert.equal(reply.type, 'snapshot');
		assert.match(reply.snapshot.endpoint_id, /^[a-f0-9]{64}$/);
		assert.equal(reply.snapshot.devices.length, 0, 'Temporary release probe unexpectedly has paired devices');
	} finally {
		await within(engine.execute(JSON.stringify({ type: 'shutdown' })), 'shutting the engine down');
	}
	console.log('Release payload verified; staged addon opened and shut down a temporary engine under Node. Packaged Electron/OS behaviour remains a later check.');
}

/** Launches the assembled application outside the checkout and checks it starts. */
export async function verifyPackage(root: string, signal: AbortSignal): Promise<void> {
	await runPackagedApplication(root, join(root, 'dist', `${process.platform}-${process.arch}`), signal);
}

/** Verifies installer checksums and, on macOS, the mounted application payload and its launch. */
export async function verifyInstallers(root: string, signal: AbortSignal): Promise<void> {
	const product = await readProduct(root);
	const directory = join(root, 'dist/installers', `${process.platform}-${process.arch}`);
	const manifest = await verifyInstallerFiles(directory, product);
	if (isLinux) {
		await verifyLinuxPackages(directory, manifest.artifacts.map(file => file.path), product, signal);
		console.log('Installer checksums and desktop integration passed. Native installation, upgrade and uninstall checks have not been performed by this command.');
		return;
	}
	if (!isMacintosh) {
		console.log('Installer checksums passed. Native installation, upgrade and uninstall checks have not been performed by this command.');
		return;
	}
	const processes = new ProcessRunner(signal);
	const work = await mkdtemp(join(tmpdir(), 'vnilink-dmg-check-'));
	const mount = join(work, 'volume');
	let mounted = false;
	try {
		const dmg = join(directory, manifest.artifacts[0].path);
		await processes.run('hdiutil', ['verify', dmg], { signal: AbortSignal.timeout(120000) });
		await mkdir(mount);
		await processes.run('hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount, dmg], { signal: AbortSignal.timeout(30000) });
		mounted = true;
		const name = `${product.nameLong}.app`;
		const prefix = `${name}/`;
		const expected = manifest.application.files
			.filter(file => file.path.startsWith(prefix))
			.map(file => ({ ...file, path: file.path.slice(prefix.length) }));
		assert(expected.length > 0, 'Installer manifest has no application payload');
		assert.deepEqual(await packageInventory(join(mount, name)), expected, 'DMG application differs from the assembled input');
		if (manifest.application.signing === 'developer-id') {
			// What Gatekeeper checks on a download: the bundle's seal, the image's signature and its stapled ticket.
			await processes.run('codesign', ['--verify', '--deep', '--strict', join(mount, name)], { signal: AbortSignal.timeout(120000) });
			await processes.run('spctl', ['--assess', '--type', 'open', '--context', 'context:primary-signature', dmg], { signal: AbortSignal.timeout(60000) });
			await processes.run('xcrun', ['stapler', 'validate', dmg], { signal: AbortSignal.timeout(60000) });
		}
		const copy = join(work, 'application');
		await mkdir(copy);
		await cp(join(mount, name), join(copy, name), { recursive: true, verbatimSymlinks: true });
		await writeFile(join(copy, 'packaging-manifest.json'), JSON.stringify(manifest.application));
		await runPackagedApplication(root, copy, signal);
		console.log('DMG integrity, exact application payload and isolated launch passed.');
	} finally {
		await processes.stop();
		// Detach uses a separate owner because cancellation closes the work runner.
		const cleanup = new ProcessRunner();
		try {
			if (mounted) {
				await cleanup.run('hdiutil', ['detach', mount], { signal: AbortSignal.timeout(15000) });
			}
			await rm(work, { recursive: true, force: true });
		} finally {
			await cleanup.stop();
		}
	}
}

/**
 * Checks what each Linux package installs for the desktop without installing
 * it. Both packages carry the one desktop entry electron-builder wrote, so the
 * Debian package's is read and the RPM's file list is checked against it.
 */
async function verifyLinuxPackages(directory: string, artifacts: readonly string[], product: Product, signal: AbortSignal): Promise<void> {
	const deb = artifacts.find(path => path.endsWith('.deb'));
	const rpm = artifacts.find(path => path.endsWith('.rpm'));
	assert(deb && rpm, 'Expected a .deb and an .rpm installer');
	const processes = new ProcessRunner(signal);
	const work = await mkdtemp(join(tmpdir(), 'vnilink-package-check-'));
	try {
		await processes.run('dpkg-deb', ['-x', join(directory, deb), work]);
		const entry = await readFile(join(work, 'usr/share/applications', product.platforms.linux.desktopFileName), 'utf8');
		// Not `packageInventory`: the package links its command to an absolute path under /opt.
		const debFiles = (await readdir(work, { recursive: true, withFileTypes: true }))
			.filter(file => !file.isDirectory())
			.map(file => relative(work, join(file.parentPath, file.name)));
		checkDesktopIntegration(product, debFiles, entry);
		const rpmFiles = (await processes.output('rpm', ['-qlp', join(directory, rpm)])).split('\n').map(path => path.replace(/^\//, '')).filter(Boolean);
		checkDesktopIntegration(product, rpmFiles, entry);
	} finally {
		await processes.stop();
		await rm(work, { recursive: true, force: true });
	}
}

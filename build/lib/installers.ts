// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Installer generation: input integrity, cancellation and publication of the host's installers. */
import assert from 'node:assert/strict';
import { access, cp, mkdir, mkdtemp, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { basename, extname, join } from 'node:path';
import * as darwin from './package.darwin.ts';
import * as linux from './package.linux.ts';
import { builderPlatformFlag, packageConfiguration, runElectronBuilder, verifyPackageFiles, type BuilderConfiguration } from './package.ts';
import * as win32 from './package.win32.ts';
import { acquireLock } from './lock.ts';
import { ProcessRunner } from './processes.ts';
import { readProduct, type Product } from './product.ts';
import { fileSha256, verifyReleaseFiles, type InstallerArtifact, type InstallerManifest, type ReleaseManifest } from './inventory.ts';
import { isMacintosh } from './platform.ts';

/** What each `package.<platform>.ts` declares about its installers. */
interface PlatformInstallers {
	readonly extensions: readonly string[];
	readonly resources: readonly string[];
	installerOptions(product: Product, base: BuilderConfiguration): Record<string, unknown>;
}

const platforms: Partial<Record<NodeJS.Platform, PlatformInstallers>> = { darwin, win32, linux };

function platformInstallers(platform: NodeJS.Platform): PlatformInstallers {
	const installers = platforms[platform];
	if (!installers) {
		throw new Error(`Unsupported installer host: ${platform}`);
	}
	return installers;
}

export function installerExtensions(platform: NodeJS.Platform): readonly string[] {
	return platformInstallers(platform).extensions;
}

/** The repository files `platform`'s installer reads, which the release stages under `packaging-resources`. */
export function installerResources(platform: NodeJS.Platform): readonly string[] {
	return platformInstallers(platform).resources;
}

/** Native-host installer targets consume an existing, verified Electron application. */
export function installerConfiguration(root: string, stage: string, output: string, product: Product, manifest: Pick<ReleaseManifest, 'platform' | 'electronVersion'>): BuilderConfiguration {
	const base = packageConfiguration(root, stage, output, product, manifest);
	return { ...base, artifactName: product.nameLong + '-${version}-${os}-${arch}.${ext}', ...platformInstallers(manifest.platform).installerOptions(product, base) };
}

export interface InstallerOptions {
	/** Runs electron-builder; replaced by tests. */
	readonly runBuilder?: (processes: ProcessRunner, args: readonly string[], cwd: string) => Promise<unknown>;
}

/** Generates `dist/installers/<platform>-<arch>` from the verified assembled application. */
export async function buildInstallers(root: string, signal: AbortSignal, options: InstallerOptions = {}): Promise<string> {
	const product = await readProduct(root);
	const target = `${process.platform}-${process.arch}`;
	const stage = join(root, '.build/release', target);
	const application = join(root, 'dist', target);
	const output = join(root, 'dist/installers', target);
	await mkdir(join(root, 'dist/installers'), { recursive: true });
	// Both assembly and installer jobs own this lock while reading the application.
	const releaseLock = await acquireLock(`${application}.lock`);
	const processes = new ProcessRunner(signal);
	let work: string | undefined;
	try {
		const release = await verifyReleaseFiles(stage);
		const manifest = await verifyPackageFiles(application, product);
		assert.equal(manifest.version, release.version);
		assert.equal(manifest.electronVersion, release.electronVersion);
		work = await mkdtemp(join(root, 'dist/installers', '.installer-'));
		const buildOutput = join(work, 'output');
		const configPath = join(work, 'builder.json');
		await writeFile(configPath, JSON.stringify(installerConfiguration(root, stage, buildOutput, product, manifest), null, 2));
		// The builder packages a copy: on Linux it adds the sandbox's AppArmor
		// profile to what it packages, and the verified application stays as verified.
		const copy = join(work, 'application');
		await cp(application, copy, { recursive: true, verbatimSymlinks: true });
		const prepackaged = isMacintosh ? join(copy, `${product.nameLong}.app`) : copy;
		const args = ['--config', configPath, '--prepackaged', prepackaged, builderPlatformFlag(), `--${process.arch}`, '--publish', 'never'];
		await (options.runBuilder ?? runElectronBuilder)(processes, args, root);
		const artifacts: InstallerArtifact[] = [];
		for (const extension of installerExtensions(process.platform)) {
			const entries = (await readdir(buildOutput, { withFileTypes: true })).filter(entry => entry.isFile() && extname(entry.name) === extension);
			assert.equal(entries.length, 1, `Expected exactly one ${extension} artifact`);
			const path = entries[0].name;
			const bytes = (await stat(join(buildOutput, path))).size;
			assert(bytes > 0);
			artifacts.push({ path, bytes, sha256: await fileSha256(join(buildOutput, path)) });
		}
		await verifyPackageFiles(application, product);
		await verifyReleaseFiles(stage);
		await writeFile(join(buildOutput, 'installer-manifest.json'), JSON.stringify({ application: manifest, artifacts }, null, 2) + '\n');
		await writeFile(join(buildOutput, 'SHA256SUMS'), artifacts.map(file => `${file.sha256}  ${file.path}\n`).join(''));
		signal.throwIfAborted();
		await publish(buildOutput, output, join(work, 'previous'));
		console.log(`Unsigned installers generated: ${output}`);
		return output;
	} finally {
		try {
			await processes.stop();
		} finally {
			if (work) {
				await rm(work, { recursive: true, force: true });
			}
			await releaseLock();
		}
	}
}

/** Replaces `output` with `built`, restoring the last completed output if publication fails. */
async function publish(built: string, output: string, previous: string): Promise<void> {
	let hadPrevious = false;
	try {
		await access(output);
		await rename(output, previous);
		hadPrevious = true;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
			throw error;
		}
	}
	try {
		await rename(built, output);
	} catch (error) {
		if (hadPrevious) {
			await rename(previous, output);
		}
		throw error;
	}
}

/** Checks installer checksums against their manifest and `SHA256SUMS`. */
export async function verifyInstallerFiles(
	directory: string,
	product: Product,
	platform: NodeJS.Platform = process.platform,
	architecture: string = process.arch,
): Promise<InstallerManifest> {
	const manifest = JSON.parse(await readFile(join(directory, 'installer-manifest.json'), 'utf8')) as InstallerManifest;
	assert.equal(manifest.application.appId, product.appId);
	assert.equal(manifest.application.platform, platform);
	assert.equal(manifest.application.architecture, architecture);
	assert.equal(manifest.application.signing, 'unsigned');
	assert.deepEqual(manifest.artifacts.map(file => extname(file.path)).sort(), [...installerExtensions(platform)].sort());
	for (const file of manifest.artifacts) {
		assert.equal(file.path, basename(file.path), 'Installer path must be a filename');
		assert(!file.path.includes('\\') && !file.path.includes('/'));
		assert.match(file.sha256, /^[a-f0-9]{64}$/);
		assert(file.bytes > 0);
		assert.equal((await stat(join(directory, file.path))).size, file.bytes);
		assert.equal(await fileSha256(join(directory, file.path)), file.sha256, `Installer checksum differs: ${file.path}`);
	}
	assert.equal(await readFile(join(directory, 'SHA256SUMS'), 'utf8'), manifest.artifacts.map(file => `${file.sha256}  ${file.path}\n`).join(''));
	return manifest;
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** The inventories and manifests every build artifact is recorded and checked with. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, readFile, readdir, readlink } from 'node:fs/promises';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';

export interface ReleaseFile {
	readonly path: string;
	readonly bytes?: number;
	readonly sha256: string;
}

export interface ReleaseManifest {
	readonly source: string;
	readonly product: string;
	readonly version: string;
	readonly electronVersion: string;
	readonly platform: NodeJS.Platform;
	readonly architecture: string;
	readonly rustTarget: string;
	readonly profile: 'release';
	readonly files: readonly ReleaseFile[];
}

export type PackageFile = { readonly path: string; readonly type: 'link'; readonly target: string } | { readonly path: string; readonly type: 'file'; readonly bytes: number; readonly executable: boolean; readonly sha256: string };

export interface PackageManifest {
	readonly appId: string;
	readonly version: string;
	readonly electronVersion: string;
	readonly packagerVersion: string;
	readonly platform: NodeJS.Platform;
	readonly architecture: string;
	readonly signing: 'unsigned';
	readonly executable: string;
	readonly asarSha256: string;
	readonly nativeSha256: string;
	readonly files: readonly PackageFile[];
}

export interface InstallerArtifact {
	readonly path: string;
	readonly bytes: number;
	readonly sha256: string;
}

export interface InstallerManifest {
	readonly application: PackageManifest;
	readonly artifacts: readonly InstallerArtifact[];
}

export async function fileSha256(path: string): Promise<string> {
	const hash = createHash('sha256');
	for await (const bytes of createReadStream(path)) {
		hash.update(bytes as Buffer);
	}
	return hash.digest('hex');
}

function portablePath(path: string): string {
	return path.split('\\').join('/');
}

/** Every regular file under `directory` with its size and checksum; anything else is refused. */
export async function releaseInventory(directory: string, current = directory): Promise<ReleaseFile[]> {
	const files: ReleaseFile[] = [];
	for (const entry of await readdir(current, { withFileTypes: true })) {
		const filename = join(current, entry.name);
		if (entry.isDirectory()) {
			files.push(...await releaseInventory(directory, filename));
		} else if (entry.isFile()) {
			const bytes = await readFile(filename);
			files.push({ path: portablePath(relative(directory, filename)), bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
		} else {
			throw new Error(`Release payload contains an unsupported file type: ${filename}`);
		}
	}
	return files.sort((left, right) => left.path.localeCompare(right.path, 'en'));
}

/** A complete Electron bundle, preserving framework symlinks as links. */
export async function packageInventory(directory: string, current = directory): Promise<PackageFile[]> {
	const files: PackageFile[] = [];
	for (const entry of await readdir(current, { withFileTypes: true })) {
		if (current === directory && entry.name === 'packaging-manifest.json') {
			continue;
		}
		const filename = join(current, entry.name);
		const path = portablePath(relative(directory, filename));
		if (entry.isDirectory()) {
			files.push(...await packageInventory(directory, filename));
		} else if (entry.isSymbolicLink()) {
			const target = await readlink(filename);
			const destination = relative(directory, resolve(dirname(filename), target));
			if (isAbsolute(target) || destination === '..' || destination.startsWith('../') || destination.startsWith('..\\') || isAbsolute(destination)) {
				throw new Error(`Packaged symlink escapes the application: ${path}`);
			}
			files.push({ path, type: 'link', target });
		} else if (entry.isFile()) {
			const metadata = await lstat(filename);
			files.push({ path, type: 'file', bytes: metadata.size, executable: (metadata.mode & 0o111) !== 0, sha256: await fileSha256(filename) });
		} else {
			throw new Error(`Unsupported packaged file: ${path}`);
		}
	}
	return files.sort((a, b) => a.path.localeCompare(b.path, 'en'));
}

/** Checks the exact staged payload against its build manifest. */
export async function verifyReleaseFiles(directory: string): Promise<ReleaseManifest> {
	const manifest = JSON.parse(await readFile(join(directory, 'build-manifest.json'), 'utf8')) as ReleaseManifest;
	assert.equal(manifest.profile, 'release', 'Expected a release profile');
	assert.equal(manifest.platform, process.platform, 'Release platform differs from the verifier host');
	assert.equal(manifest.architecture, process.arch, 'Release architecture differs from the verifier host');
	const actual = (await releaseInventory(directory)).filter(file => file.path !== 'build-manifest.json');
	assert.deepEqual(actual, manifest.files, 'Release inventory differs: files missing, added or modified');
	return manifest;
}

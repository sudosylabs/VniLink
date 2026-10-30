// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Whether `postinstall` already prepared this checkout for its current
 * inputs, after VS Code's `build/npm/installStateHash.ts`: a hash of the
 * files that decide the dependencies, the Rust toolchain and the branded
 * Electron, and of the Node version, saved in `node_modules`.
 */
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

/** Files whose content decides what `postinstall` prepares. */
export const inputFiles = ['package.json', 'package-lock.json', '.nvmrc', 'rust-toolchain.toml', 'product.json', 'build/prerequisites.json'];

const packageJsonRelevantKeys = ['name', 'dependencies', 'devDependencies', 'optionalDependencies', 'overrides', 'engines'];

export interface InstallState {
	readonly nodeVersion: string;
	readonly fileHashes: Readonly<Record<string, string>>;
}

export function stateFile(root: string): string {
	return join(root, 'node_modules', '.postinstall-state');
}

/** `package.json` scripts and the lockfile's own version do not change what is installed. */
function normalize(name: string, raw: string): string {
	if (name === 'package.json') {
		const json = JSON.parse(raw) as Record<string, unknown>;
		return JSON.stringify(Object.fromEntries(packageJsonRelevantKeys.filter(key => Object.hasOwn(json, key)).map(key => [key, json[key]])));
	}
	if (name === 'package-lock.json') {
		const json = JSON.parse(raw) as { version?: unknown; packages?: Record<string, { version?: unknown }> };
		delete json.version;
		delete json.packages?.['']?.version;
		return JSON.stringify(json);
	}
	return raw;
}

export async function computeState(root: string): Promise<InstallState> {
	const fileHashes: Record<string, string> = {};
	for (const name of inputFiles) {
		const raw = await readFile(join(root, name), 'utf8').catch(() => undefined);
		if (raw !== undefined) {
			fileHashes[name] = createHash('sha256').update(normalize(name, raw)).digest('hex');
		}
	}
	return { nodeVersion: process.versions.node, fileHashes };
}

export async function isUpToDate(root: string): Promise<boolean> {
	const saved = await readFile(stateFile(root), 'utf8').catch(() => undefined);
	return saved !== undefined && saved === JSON.stringify(await computeState(root));
}

export async function saveState(root: string): Promise<void> {
	await writeFile(stateFile(root), JSON.stringify(await computeState(root)));
}

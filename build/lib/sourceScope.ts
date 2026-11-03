// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { readFile, readdir } from 'node:fs/promises';
import { posix, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { manifestPath, type VsCodeManifest } from './vscode/manifest.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

// Read as the module loads, with top-level await: the lint and format configurations import it.
const manifest = JSON.parse(await readFile(resolve(root, manifestPath), 'utf8')) as VsCodeManifest;

// Verbatim and pruned copies, and adapted infrastructure marked `upstreamStyle`, keep VS Code's
// formatting and lint baseline. Other adaptations are VniLink code held to application rules.
export const upstreamFiles = manifest.files.filter(file => !file.adapted || file.upstreamStyle === true).map(file => file.path);

const upstream = new Set(upstreamFiles);

const applicationDirectories = new Set<string>();

async function findApplicationDirectories(directory: string): Promise<void> {
	for (const entry of await readdir(resolve(root, directory), { withFileTypes: true })) {
		const path = `${directory}/${entry.name}`;
		if (entry.isDirectory()) {
			await findApplicationDirectories(path);
		} else if (!upstream.has(path)) {
			let parent = posix.dirname(path);
			while (parent !== '.') {
				applicationDirectories.add(parent);
				parent = posix.dirname(parent);
			}
		}
	}
}

await findApplicationDirectories('src');

export const upstreamIgnores = [...new Set(upstreamFiles.map(path => {
	const segments = path.split('/');
	for (let index = 1; index < segments.length; index++) {
		const directory = segments.slice(0, index).join('/');
		if (!applicationDirectories.has(directory)) {
			return `${directory}/**`;
		}
	}
	return path;
}))];

// Copied build tooling (ESLint rules) keeps upstream formatting unless adapted.
const upstreamTooling = new Set(manifest.tooling.filter(file => !file.adapted).map(file => file.path));

export function isApplicationFile(filename: string): boolean {
	const path = relative(root, resolve(filename)).split(sep).join('/');
	return !upstream.has(path) && !upstreamTooling.has(path) && path !== manifestPath;
}

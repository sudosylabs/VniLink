// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * `build/vscode-source.json`: the pinned VS Code commit and the provenance of
 * every copied file. A copy is one of three kinds:
 *
 * - verbatim: upstream text after `flattenSource`, byte for byte;
 * - pruned: verbatim text with whole unused declarations deleted (`pruned` names them);
 * - adapted: reviewed edits, described by `adaptation` or the file's header.
 */
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { processOutput } from '../processes.ts';


/** The manifest's path relative to the repository root. */
export const manifestPath = 'build/vscode-source.json';

export interface ManifestFile {
	path: string;
	upstreamPath: string;
	/** The local file's hash. */
	sha256: string;
	/** Declarations deleted from the upstream text, as `name` or `Owner.member`. */
	pruned?: string[];
	adapted?: boolean;
	/** An adapted file that keeps VS Code's formatting and lint baseline. */
	upstreamStyle?: boolean;
	adaptation?: string;
}

/** A copied build or lint file; these are outside the `src` dependency graph. */
export interface ToolingFile {
	path: string;
	upstreamPath: string;
	sha256: string;
	adapted?: boolean;
}

export interface DependencyEdge {
	readonly from: string;
	readonly to: string;
	readonly kind: string;
}

export interface VsCodeManifest {
	repository: string;
	ref: string;
	commit: string;
	sourceModifications: string;
	runtimeIntegrated: boolean;
	scope: string;
	files: ManifestFile[];
	tooling: ToolingFile[];
	roots: string[];
	entryPoints: string[];
	compilerSupport: string[];
	applicationDependencies: string[];
	dependencyEdges: DependencyEdge[];
	externalDependencies: string[];
}

export type CopyKind = 'verbatim' | 'pruned' | 'adapted';

export function copyKind(file: ManifestFile | ToolingFile): CopyKind {
	if (file.adapted) {
		return 'adapted';
	}
	return 'pruned' in file && file.pruned?.length ? 'pruned' : 'verbatim';
}

/** Maps an upstream `src/vs/...` path to its repository path under `src/`. */
export function localPath(upstreamPath: string): string {
	if (!upstreamPath.startsWith('src/vs/')) {
		throw new Error(`Not an upstream source path: ${upstreamPath}`);
	}
	return `src/${upstreamPath.slice('src/vs/'.length)}`;
}

/** The declared mechanical change every copy carries: the license file's name and flattened import paths. */
export function flattenSource(content: string): string {
	return content
		.replace('See License.txt in the project root', 'See LICENSE.vscode.txt in the project root')
		.replace(/(?<!src\/)\bvs\/(base|platform|workbench|code|editor)\//g, '$1/');
}

export function sha256(content: string | Buffer): string {
	return createHash('sha256').update(content).digest('hex');
}

export async function readManifest(root: string): Promise<VsCodeManifest> {
	return JSON.parse(await readFile(join(root, manifestPath), 'utf8')) as VsCodeManifest;
}

export async function writeManifestFile(root: string, manifest: VsCodeManifest): Promise<void> {
	manifest.files.sort((left, right) => left.path.localeCompare(right.path, 'en'));
	manifest.tooling.sort((left, right) => left.path.localeCompare(right.path, 'en'));
	await writeFile(join(root, manifestPath), `${JSON.stringify(manifest, null, 2)}\n`);
}

/** Refuses an upstream checkout that is not at the manifest's commit. */
export async function assertCheckout(manifest: VsCodeManifest, checkout: string): Promise<void> {
	const stdout = await processOutput('git', ['-C', checkout, 'rev-parse', 'HEAD']);
	if (stdout.trim() !== manifest.commit) {
		throw new Error(`Upstream checkout ${checkout} is at ${stdout.trim()}; the manifest pins ${manifest.commit} (${manifest.ref})`);
	}
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Copies pinned VS Code sources with their dependency closure and records their provenance. */
import { access, mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, join, posix } from 'node:path';
import { companionsOf, derivedContent, provenanceGraph, sourceDependencies } from './dependencies.ts';
import { assertCheckout, flattenSource, localPath, readManifest, sha256, writeManifestFile, type DependencyEdge, type VsCodeManifest } from './manifest.ts';
import { pruneSource } from './prune.ts';

function exists(path: string): Promise<boolean> {
	return access(path).then(() => true, () => false);
}

/** The first of `candidates` that exists under `directory`. */
async function firstExisting(directory: string, candidates: readonly string[]): Promise<string | undefined> {
	for (const candidate of candidates) {
		if (await exists(join(directory, candidate))) {
			return candidate;
		}
	}
	return undefined;
}

function resolveUpstream(checkout: string, from: string, specifier: string): Promise<string | undefined> {
	const base = posix.normalize(posix.join(posix.dirname(from), specifier.split(/[?#]/)[0]));
	return firstExisting(checkout, base.endsWith('.js') ? [base.replace(/\.js$/, '.ts'), base, base.replace(/\.js$/, '.d.ts')] : [base, `${base}.ts`]);
}

async function checkedManifest(root: string, checkout: string): Promise<VsCodeManifest> {
	const manifest = await readManifest(root);
	await assertCheckout(manifest, checkout);
	return manifest;
}

/** Resolves an import from a copied file to an existing application file, if there is one. */
async function applicationTarget(root: string, { from, specifier }: { readonly from: string; readonly specifier: string }): Promise<string | undefined> {
	if (!specifier.startsWith('.')) {
		return undefined;
	}
	const base = posix.normalize(posix.join(posix.dirname(from), specifier));
	return firstExisting(root, [base.replace(/\.js$/, '.ts'), base]);
}

/** Copies that application code imports directly; they are the graph's entry points. */
async function applicationImports(root: string, copies: ReadonlySet<string>): Promise<Set<string>> {
	const imported = new Set<string>();
	const pending = ['src'];
	for (let directory = pending.pop(); directory !== undefined; directory = pending.pop()) {
		for (const entry of await readdir(join(root, directory), { withFileTypes: true })) {
			const path = `${directory}/${entry.name}`;
			if (entry.isDirectory()) {
				pending.push(path);
			} else if (path.endsWith('.ts') && !copies.has(path)) {
				for (const { specifier } of sourceDependencies(path, await readFile(join(root, path), 'utf8'))) {
					const target = await applicationTarget(root, { from: path, specifier });
					if (target && copies.has(target)) {
						imported.add(target);
					}
				}
			}
		}
	}
	return imported;
}

/** Re-derives the graph's entry points, boundary and edges, then writes the manifest. */
async function writeManifest(root: string, manifest: VsCodeManifest): Promise<void> {
	// A recorded copy is part of the graph, never its application boundary.
	const copies = new Set(manifest.files.map(file => file.path));
	const compilerSupport = new Set(manifest.compilerSupport);
	// A copied test is run by the unit-test runner, so it is an entry point like application code that imports a copy.
	const tests = [...copies].filter(path => path.endsWith('.test.ts'));
	manifest.entryPoints = [...new Set([...await applicationImports(root, copies), ...tests])].filter(path => !compilerSupport.has(path)).sort();
	manifest.roots = [...manifest.entryPoints, ...compilerSupport].sort();
	manifest.applicationDependencies = manifest.applicationDependencies.filter(path => !copies.has(path));
	let graph = await provenanceGraph(root, manifest);
	// Adapted copies may import application modules; record those as the graph's boundary.
	const application = (await Promise.all(graph.unresolved.map(dependency => applicationTarget(root, dependency)))).filter(path => path !== undefined);
	if (application.length) {
		manifest.applicationDependencies = [...new Set([...manifest.applicationDependencies, ...application])];
		graph = await provenanceGraph(root, manifest);
	}
	if (graph.unresolved.length) {
		throw new Error(`Unresolved dependencies: ${JSON.stringify(graph.unresolved)}`);
	}
	// Forget application modules that no copied file imports any more.
	const imported = new Set(graph.edges.map((edge: DependencyEdge) => edge.to));
	manifest.applicationDependencies = manifest.applicationDependencies.filter(path => imported.has(path));
	manifest.dependencyEdges = graph.edges;
	manifest.externalDependencies = graph.external;
	await writeManifestFile(root, manifest);
}

/**
 * Copies `roots` and every upstream file they reach that the repository does
 * not already contain. An application module at an upstream path (such as
 * `defaultStyles.ts`) is the import's boundary: it is neither overwritten nor
 * followed, and is reported in `boundaries`.
 */
export async function importSources(root: string, checkout: string, roots: readonly string[]): Promise<{ copied: string[]; boundaries: string[] }> {
	const manifest = await checkedManifest(root, checkout);
	const known = new Set(manifest.files.map(file => file.path));
	const copies = new Map<string, { readonly upstreamPath: string; readonly content: string }>();
	const boundaries = new Set<string>();
	const pending = [...roots];
	const seen = new Set<string>();
	for (let path = pending.pop(); path !== undefined; path = pending.pop()) {
		if (seen.has(path)) {
			continue;
		}
		seen.add(path);
		const target = localPath(path);
		const content = await readFile(join(checkout, path), 'utf8');
		if (!known.has(target)) {
			const flattened = /\.(ts|js|css)$/.test(path) ? flattenSource(content) : content;
			if (await exists(join(root, target)) && await readFile(join(root, target), 'utf8') !== flattened) {
				if (roots.includes(path)) {
					throw new Error(`${target} exists but is not a recorded upstream copy`);
				}
				// An application module owns this path; copies import it as it is.
				boundaries.add(target);
				continue;
			}
			copies.set(target, { upstreamPath: path, content: flattened });
		}
		for (const { specifier } of sourceDependencies(path, content)) {
			if (!specifier.startsWith('.')) {
				continue;
			}
			const dependency = await resolveUpstream(checkout, path, specifier);
			if (!dependency) {
				throw new Error(`Unresolved upstream import ${specifier} from ${path}`);
			}
			pending.push(dependency);
		}
		for (const companion of companionsOf(path)) {
			if (await exists(join(checkout, companion))) {
				pending.push(companion);
			}
		}
	}
	// Write only after the whole closure resolved, so a failure copies nothing.
	const copied = [...copies.keys()];
	for (const [target, { upstreamPath, content }] of copies) {
		await mkdir(dirname(join(root, target)), { recursive: true });
		await writeFile(join(root, target), content);
		manifest.files.push({ path: target, upstreamPath, sha256: sha256(content) });
	}
	await writeManifest(root, manifest);
	return { copied, boundaries: [...boundaries].sort() };
}

/** Re-derives entry points and dependency edges after application imports change. */
export async function syncManifest(root: string): Promise<void> {
	await writeManifest(root, await readManifest(root));
}

/**
 * Records reviewed edits to copied files as declared adaptations. With
 * `upstreamStyle`, the files keep VS Code's formatting and lint baseline.
 */
export async function recordAdaptations(root: string, paths: readonly string[], { upstreamStyle = false } = {}): Promise<void> {
	const manifest = await readManifest(root);
	for (const path of paths) {
		const file = manifest.files.find(candidate => candidate.path === path) ?? manifest.tooling.find(candidate => candidate.path === path);
		if (!file) {
			throw new Error(`${path} is not a recorded upstream copy`);
		}
		file.sha256 = sha256(await readFile(join(root, path)));
		file.adapted = true;
		if (upstreamStyle) {
			Object.assign(file, { upstreamStyle: true });
		}
		if ('pruned' in file) {
			delete file.pruned;
		}
	}
	await writeManifest(root, manifest);
}

/**
 * Deletes whole declarations from verbatim or pruned copies: `path` is
 * re-derived from upstream without every name pruned so far plus `names`.
 */
export async function pruneCopies(root: string, checkout: string, path: string, names: readonly string[]): Promise<void> {
	const manifest = await checkedManifest(root, checkout);
	const file = manifest.files.find(candidate => candidate.path === path);
	if (!file) {
		throw new Error(`${path} is not a recorded upstream copy`);
	}
	if (file.adapted) {
		// An adapted file's text is its own; delete from it directly.
		const pruned = pruneSource(path, await readFile(join(root, path), 'utf8'), names);
		await writeFile(join(root, path), pruned);
		file.sha256 = sha256(pruned);
	} else {
		const pruned = [...new Set([...(file.pruned ?? []), ...names])].sort();
		const content = await derivedContent({ ...file, pruned }, checkout);
		await writeFile(join(root, path), content);
		file.pruned = pruned;
		file.sha256 = sha256(content);
	}
	await writeManifest(root, manifest);
}

/** Deletes copies that nothing imports any more, with their manifest entries. */
export async function removeCopies(root: string, paths: readonly string[]): Promise<void> {
	const manifest = await readManifest(root);
	for (const path of paths) {
		const index = manifest.files.findIndex(candidate => candidate.path === path);
		if (index < 0) {
			throw new Error(`${path} is not a recorded upstream copy`);
		}
		manifest.files.splice(index, 1);
		await rm(join(root, path));
	}
	await writeManifest(root, manifest);
}

/** Replaces copied files with their pinned upstream content, discarding any adaptation or pruning. */
export async function resetToUpstream(root: string, checkout: string, paths: readonly string[]): Promise<void> {
	const manifest = await checkedManifest(root, checkout);
	for (const path of paths) {
		const index = manifest.files.findIndex(candidate => candidate.path === path);
		if (index < 0) {
			throw new Error(`${path} is not a recorded upstream copy`);
		}
		const { upstreamPath } = manifest.files[index];
		const content = await derivedContent({ path, upstreamPath, sha256: '' }, checkout);
		await writeFile(join(root, path), content);
		manifest.files[index] = { path, upstreamPath, sha256: sha256(content) };
	}
	await writeManifest(root, manifest);
}

const usage = `Usage:
  node build/run.ts vscode import <checkout> <src/vs/...> [...]
  node build/run.ts vscode adapt [--upstream-style] <src/...> [...]
  node build/run.ts vscode prune <checkout> <src/...> <name | Owner.member> [...]
  node build/run.ts vscode remove <src/...> [...]
  node build/run.ts vscode reset <checkout> <src/...> [...]
  node build/run.ts vscode sync
<checkout> is a VS Code checkout at the commit build/vscode-source.json pins.`;

/** The `vscode` task: copies, records, prunes, removes, resets or re-derives upstream sources. */
export async function vscodeCommand(root: string, args: readonly string[]): Promise<void> {
	const [command, ...rest] = args;
	if (command === 'import' && rest.length > 1) {
		const { copied, boundaries } = await importSources(root, rest[0], rest.slice(1));
		console.log(`Copied ${String(copied.length)} upstream files.`);
		for (const boundary of boundaries) {
			console.log(`Kept application module ${boundary}; check that it provides what the copies import.`);
		}
	} else if (command === 'adapt' && rest.length) {
		const upstreamStyle = rest.includes('--upstream-style');
		const paths = rest.filter(arg => arg !== '--upstream-style');
		await recordAdaptations(root, paths, { upstreamStyle });
		console.log(`Recorded ${String(paths.length)} adaptations.`);
	} else if (command === 'prune' && rest.length > 2) {
		await pruneCopies(root, rest[0], rest[1], rest.slice(2));
		console.log(`Pruned ${String(rest.length - 2)} declarations from ${rest[1]}.`);
	} else if (command === 'remove' && rest.length) {
		await removeCopies(root, rest);
		console.log(`Removed ${String(rest.length)} copies.`);
	} else if (command === 'sync' && !rest.length) {
		await syncManifest(root);
		console.log('Provenance graph updated.');
	} else if (command === 'reset' && rest.length > 1) {
		await resetToUpstream(root, rest[0], rest.slice(1));
		console.log(`Reset ${String(rest.length - 1)} files to upstream.`);
	} else {
		throw new Error(usage);
	}
}

/** Copies pinned VS Code sources with their dependency closure and records their provenance. */
import ts from 'typescript';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, posix } from 'node:path';
import { execFileSync } from 'node:child_process';
import { titlebarDependencies, type DependencyEdge, type VsCodeManifest } from './vscodeDependencies.ts';

const root = new URL('../', import.meta.url);

const manifestUrl = new URL('./vscode-source.json', import.meta.url);

const tsconfigUrl = new URL('../src/tsconfig.json', import.meta.url);

/** Maps an upstream `src/vs/...` path to its repository path under `src/`. */
export function localPath(upstreamPath: string): string {
	if (!upstreamPath.startsWith('src/vs/')) {
		throw new Error(`Not an upstream source path: ${upstreamPath}`);
	}
	return `src/${upstreamPath.slice('src/vs/'.length)}`;
}

/** Applies the declared mechanical import flattening to upstream content. */
export function flattenSource(content: string): string {
	return content
		.replace('See License.txt in the project root', 'See LICENSE.vscode.txt in the project root')
		.replace(/\bvs\/(base|platform|workbench|code|editor)\//g, '$1/');
}

function dependencies(path: string, content: string): string[] {
	if (path.endsWith('.css')) {
		return [...content.matchAll(/url\(\s*['"]?([^'"\s)]+)['"]?\s*\)|@import\s+['"]([^'"]+)['"]/g)]
			.map(match => match[1] || match[2])
			.filter(specifier => !/^(data:|https?:|#)/.test(specifier));
	}
	if (!/\.[cm]?[jt]sx?$/.test(path)) {
		return [];
	}
	return ts.preProcessFile(content, true, true).importedFiles.map(file => file.fileName).filter(specifier => specifier.startsWith('.'));
}

function resolveUpstream(upstream: URL, from: string, specifier: string): string | undefined {
	const base = posix.normalize(posix.join(posix.dirname(from), specifier.split(/[?#]/)[0]));
	const candidates = base.endsWith('.js') ? [base.replace(/\.js$/, '.ts'), base, base.replace(/\.js$/, '.d.ts')] : [base, `${base}.ts`];
	return candidates.find(candidate => existsSync(new URL(candidate, upstream)));
}

async function readManifest(upstream?: URL): Promise<VsCodeManifest> {
	const manifest = JSON.parse(await readFile(manifestUrl, 'utf8')) as VsCodeManifest;
	if (upstream) {
		const commit = execFileSync('git', ['-C', upstream.pathname, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
		if (commit !== manifest.commit) {
			throw new Error(`Upstream checkout is at ${commit}; the manifest pins ${manifest.commit}`);
		}
	}
	return manifest;
}

/** Resolves an import from a copied file to an existing application file, if there is one. */
function applicationTarget({ from, specifier }: { readonly from: string; readonly specifier: string }): string | undefined {
	if (!specifier.startsWith('.')) {
		return undefined;
	}
	const base = posix.normalize(posix.join(posix.dirname(from), specifier));
	return [base.replace(/\.js$/, '.ts'), base].find(candidate => existsSync(new URL(candidate, root)));
}

/** Copies that application code imports directly; they are the graph's entry points. */
async function applicationImports(copies: ReadonlySet<string>): Promise<Set<string>> {
	const imported = new Set<string>();
	const pending = ['src'];
	while (pending.length) {
		const directory = pending.pop() ?? '';
		for (const entry of await readdir(new URL(`${directory}/`, root), { withFileTypes: true })) {
			const path = `${directory}/${entry.name}`;
			if (entry.isDirectory()) {
				pending.push(path);
			} else if (path.endsWith('.ts') && !copies.has(path)) {
				for (const specifier of dependencies(path, await readFile(new URL(path, root), 'utf8'))) {
					const target = applicationTarget({ from: path, specifier });
					if (target && copies.has(target)) {
						imported.add(target);
					}
				}
			}
		}
	}
	return imported;
}

async function writeManifest(manifest: VsCodeManifest): Promise<void> {
	manifest.files.sort((left, right) => left.path.localeCompare(right.path, 'en'));
	// A recorded copy is part of the graph, never its application boundary.
	const copies = new Set(manifest.files.map(file => file.path));
	const compilerSupport = new Set(manifest.compilerSupport ?? []);
	manifest.entryPoints = [...await applicationImports(copies)].filter(path => !compilerSupport.has(path)).sort();
	manifest.roots = [...manifest.entryPoints, ...compilerSupport].sort();
	manifest.applicationDependencies = manifest.applicationDependencies.filter(path => !copies.has(path));
	let graph = await titlebarDependencies(manifest);
	// Adapted copies may import application modules; record those as the graph's boundary.
	const application = graph.unresolved.map(applicationTarget).filter(path => path !== undefined);
	if (application.length) {
		manifest.applicationDependencies = [...new Set([...manifest.applicationDependencies, ...application])];
		graph = await titlebarDependencies(manifest);
	}
	if (graph.unresolved.length) {
		throw new Error(`Unresolved dependencies: ${JSON.stringify(graph.unresolved)}`);
	}
	// Forget application modules that no copied file imports any more.
	const imported = new Set(graph.edges.map((edge: DependencyEdge) => edge.to));
	manifest.applicationDependencies = manifest.applicationDependencies.filter(path => imported.has(path));
	manifest.dependencyEdges = graph.edges;
	manifest.externalDependencies = graph.external;
	await writeFile(manifestUrl, `${JSON.stringify(manifest, null, 2)}\n`);
}

function sha256(content: string | Buffer): string {
	return createHash('sha256').update(content).digest('hex');
}

function checkoutUrl(directory: string): URL {
	return new URL(`${directory.replace(/\/$/, '')}/`, 'file://');
}

/** Copies `roots` and every upstream file they reach that the repository does not already contain. */
export async function importSources(upstreamDirectory: string, roots: readonly string[]): Promise<string[]> {
	const upstream = checkoutUrl(upstreamDirectory);
	const manifest = await readManifest(upstream);
	const known = new Set(manifest.files.map(file => file.path));
	const copies = new Map<string, { readonly upstreamPath: string; readonly content: string }>();
	const pending = [...roots];
	const seen = new Set<string>();
	while (pending.length) {
		const path = pending.pop();
		if (path === undefined || seen.has(path)) {
			continue;
		}
		seen.add(path);
		const target = localPath(path);
		const content = await readFile(new URL(path, upstream), 'utf8');
		if (!known.has(target)) {
			const flattened = /\.(ts|js|css)$/.test(path) ? flattenSource(content) : content;
			if (existsSync(new URL(target, root)) && await readFile(new URL(target, root), 'utf8') !== flattened) {
				// An application file owns this path; never overwrite it silently.
				throw new Error(`${target} exists but is not a recorded upstream copy`);
			}
			copies.set(target, { upstreamPath: path, content: flattened });
		}
		for (const specifier of dependencies(path, content)) {
			const dependency = resolveUpstream(upstream, path, specifier);
			if (!dependency) {
				throw new Error(`Unresolved upstream import ${specifier} from ${path}`);
			}
			pending.push(dependency);
		}
		if (path.endsWith('.js')) {
			// Vendored JavaScript ships with its declarations and attribution.
			for (const companion of [path.replace(/\.js$/, '.d.ts'), path.replace(/\.js$/, '.license.txt'), `${posix.dirname(path)}/cgmanifest.json`]) {
				if (existsSync(new URL(companion, upstream))) {
					pending.push(companion);
				}
			}
		}
	}
	// Write only after the whole closure resolved, so a failure copies nothing.
	const copied = [...copies.keys()];
	for (const [target, { upstreamPath, content }] of copies) {
		await mkdir(dirname(new URL(target, root).pathname), { recursive: true });
		await writeFile(new URL(target, root), content);
		manifest.files.push({ path: target, upstreamPath, sha256: sha256(content), changedImports: 0 });
	}
	await writeManifest(manifest);
	const tsconfig = JSON.parse(await readFile(tsconfigUrl, 'utf8')) as { files: string[] };
	for (const path of copied.filter(path => /\.tsx?$/.test(path))) {
		const entry = `./${path.slice('src/'.length)}`;
		if (!tsconfig.files.includes(entry)) {
			tsconfig.files.push(entry);
		}
	}
	await writeFile(tsconfigUrl, `${JSON.stringify(tsconfig, null, 2)}\n`);
	return copied;
}

/** Re-derives entry points and dependency edges after application imports change. */
export async function syncManifest(): Promise<void> {
	await writeManifest(await readManifest());
}

/**
 * Records reviewed edits to copied files as declared adaptations. With
 * `upstreamStyle`, the files keep VS Code's formatting and lint baseline.
 */
export async function recordAdaptations(paths: readonly string[], { upstreamStyle = false } = {}): Promise<void> {
	const manifest = await readManifest();
	for (const path of paths) {
		const file = manifest.files.find(candidate => candidate.path === path);
		if (!file) {
			throw new Error(`${path} is not a recorded upstream copy`);
		}
		file.sha256 = sha256(await readFile(new URL(path, root)));
		file.adapted = true;
		if (upstreamStyle) {
			file.upstreamStyle = true;
		}
		delete file.changedImports;
	}
	await writeManifest(manifest);
}

/** Replaces copied files with their pinned upstream content, discarding any adaptation. */
export async function resetToUpstream(upstreamDirectory: string, paths: readonly string[]): Promise<void> {
	const upstream = checkoutUrl(upstreamDirectory);
	const manifest = await readManifest(upstream);
	for (const path of paths) {
		const file = manifest.files.find(candidate => candidate.path === path);
		if (!file) {
			throw new Error(`${path} is not a recorded upstream copy`);
		}
		const flattened = flattenSource(await readFile(new URL(file.upstreamPath, upstream), 'utf8'));
		await writeFile(new URL(path, root), flattened);
		manifest.files[manifest.files.indexOf(file)] = { path, upstreamPath: file.upstreamPath, sha256: sha256(flattened), changedImports: 0 };
	}
	await writeManifest(manifest);
}

const usage = `Usage:
  node build/run.ts vscode import <checkout at the pinned commit> <src/vs/...> [...]
  node build/run.ts vscode adapt [--upstream-style] <src/...> [...]
  node build/run.ts vscode reset <checkout at the pinned commit> <src/...> [...]
  node build/run.ts vscode sync`;

/** The `vscode` command: copies, records, resets or re-derives upstream sources. */
export async function vscodeCommand(args: readonly string[]): Promise<void> {
	const [command, ...rest] = args;
	if (command === 'import' && rest.length > 1) {
		const copied = await importSources(rest[0], rest.slice(1));
		console.log(`Copied ${String(copied.length)} upstream files.`);
	} else if (command === 'adapt' && rest.length) {
		const upstreamStyle = rest.includes('--upstream-style');
		const paths = rest.filter(arg => arg !== '--upstream-style');
		await recordAdaptations(paths, { upstreamStyle });
		console.log(`Recorded ${String(paths.length)} adaptations.`);
	} else if (command === 'sync' && !rest.length) {
		await syncManifest();
		console.log('Provenance graph updated.');
	} else if (command === 'reset' && rest.length > 1) {
		await resetToUpstream(rest[0], rest.slice(1));
		console.log(`Reset ${String(rest.length - 1)} files to upstream.`);
	} else {
		throw new Error(usage);
	}
}

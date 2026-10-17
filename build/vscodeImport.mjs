/** Copies pinned VS Code sources with their dependency closure and records their provenance. */
import ts from 'typescript';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, posix } from 'node:path';
import { execFileSync } from 'node:child_process';
import { titlebarDependencies } from './vscodeDependencies.mjs';

const root = new URL('../', import.meta.url);

const manifestUrl = new URL('./vscode-source.json', import.meta.url);

const tsconfigUrl = new URL('../src/tsconfig.json', import.meta.url);

/** Maps an upstream `src/vs/...` path to its repository path under `src/`. */
export function localPath(upstreamPath) {
	if (!upstreamPath.startsWith('src/vs/')) {
		throw new Error(`Not an upstream source path: ${upstreamPath}`);
	}
	return `src/${upstreamPath.slice('src/vs/'.length)}`;
}

/** Applies the declared mechanical import flattening to upstream content. */
export function flattenSource(content) {
	return content
		.replace('See License.txt in the project root', 'See LICENSE.vscode.txt in the project root')
		.replace(/\bvs\/(base|platform|workbench|code|editor)\//g, '$1/');
}

function dependencies(path, content) {
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

function resolveUpstream(upstream, from, specifier) {
	const base = posix.normalize(posix.join(posix.dirname(from), specifier.split(/[?#]/)[0]));
	const candidates = base.endsWith('.js') ? [base.replace(/\.js$/, '.ts'), base, base.replace(/\.js$/, '.d.ts')] : [base, `${base}.ts`];
	return candidates.find(candidate => existsSync(new URL(candidate, upstream)));
}

async function readManifest(upstream) {
	const manifest = JSON.parse(await readFile(manifestUrl, 'utf8'));
	if (upstream) {
		const commit = execFileSync('git', ['-C', upstream.pathname, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
		if (commit !== manifest.commit) {
			throw new Error(`Upstream checkout is at ${commit}; the manifest pins ${manifest.commit}`);
		}
	}
	return manifest;
}

/** Resolves an import from a copied file to an existing application file, if there is one. */
function applicationTarget({ from, specifier }) {
	if (!specifier.startsWith('.')) {
		return undefined;
	}
	const base = posix.normalize(posix.join(posix.dirname(from), specifier));
	return [base.replace(/\.js$/, '.ts'), base].find(candidate => existsSync(new URL(candidate, root)));
}

async function writeManifest(manifest) {
	manifest.files.sort((left, right) => left.path.localeCompare(right.path, 'en'));
	let graph = await titlebarDependencies(manifest);
	// Adapted copies may import application modules; record those as the graph's boundary.
	const application = graph.unresolved.map(applicationTarget).filter(Boolean);
	if (application.length) {
		manifest.applicationDependencies = [...new Set([...manifest.applicationDependencies, ...application])];
		graph = await titlebarDependencies(manifest);
	}
	if (graph.unresolved.length) {
		throw new Error(`Unresolved dependencies: ${JSON.stringify(graph.unresolved)}`);
	}
	// Forget application modules that no copied file imports any more.
	const imported = new Set(graph.edges.map(edge => edge.to));
	manifest.applicationDependencies = manifest.applicationDependencies.filter(path => imported.has(path));
	manifest.dependencyEdges = graph.edges;
	manifest.externalDependencies = graph.external;
	await writeFile(manifestUrl, `${JSON.stringify(manifest, null, 2)}\n`);
}

function sha256(content) {
	return createHash('sha256').update(content).digest('hex');
}

function checkoutUrl(directory) {
	return new URL(`${directory.replace(/\/$/, '')}/`, 'file://');
}

/** Copies `roots` and every upstream file they reach that the repository does not already contain. */
export async function importSources(upstreamDirectory, roots) {
	const upstream = checkoutUrl(upstreamDirectory);
	const manifest = await readManifest(upstream);
	const known = new Set(manifest.files.map(file => file.path));
	const copied = [];
	const pending = [...roots];
	const seen = new Set();
	while (pending.length) {
		const path = pending.pop();
		if (seen.has(path)) {
			continue;
		}
		seen.add(path);
		const target = localPath(path);
		const content = await readFile(new URL(path, upstream), 'utf8');
		if (!known.has(target)) {
			if (existsSync(new URL(target, root))) {
				// An application file owns this path; never overwrite it silently.
				throw new Error(`${target} exists but is not a recorded upstream copy`);
			}
			const flattened = flattenSource(content);
			await mkdir(dirname(new URL(target, root).pathname), { recursive: true });
			await writeFile(new URL(target, root), flattened);
			manifest.files.push({ path: target, upstreamPath: path, sha256: sha256(flattened), changedImports: 0 });
			known.add(target);
			copied.push(target);
		}
		for (const specifier of dependencies(path, content)) {
			const dependency = resolveUpstream(upstream, path, specifier);
			if (!dependency) {
				throw new Error(`Unresolved upstream import ${specifier} from ${path}`);
			}
			pending.push(dependency);
		}
	}
	for (const path of roots.map(localPath)) {
		if (!manifest.entryPoints.includes(path)) {
			manifest.entryPoints.push(path);
		}
		if (!manifest.roots.includes(path)) {
			manifest.roots.push(path);
			manifest.roots.sort();
		}
	}
	await writeManifest(manifest);
	const tsconfig = JSON.parse(await readFile(tsconfigUrl, 'utf8'));
	for (const path of copied.filter(path => /\.tsx?$/.test(path))) {
		const entry = `./${path.slice('src/'.length)}`;
		if (!tsconfig.files.includes(entry)) {
			tsconfig.files.push(entry);
		}
	}
	await writeFile(tsconfigUrl, `${JSON.stringify(tsconfig, null, 2)}\n`);
	return copied;
}

/** Records reviewed edits to copied files as declared adaptations. */
export async function recordAdaptations(paths) {
	const manifest = await readManifest();
	for (const path of paths) {
		const file = manifest.files.find(candidate => candidate.path === path);
		if (!file) {
			throw new Error(`${path} is not a recorded upstream copy`);
		}
		file.sha256 = sha256(await readFile(new URL(path, root)));
		file.adapted = true;
		delete file.changedImports;
	}
	await writeManifest(manifest);
}

/** Replaces copied files with their pinned upstream content, discarding any adaptation. */
export async function resetToUpstream(upstreamDirectory, paths) {
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
  node build/vscodeImport.mjs import <checkout at the pinned commit> <src/vs/...> [...]
  node build/vscodeImport.mjs adapt <src/...> [...]
  node build/vscodeImport.mjs reset <checkout at the pinned commit> <src/...> [...]`;

if (process.argv[1] === new URL(import.meta.url).pathname) {
	const [command, ...args] = process.argv.slice(2);
	if (command === 'import' && args.length > 1) {
		const copied = await importSources(args[0], args.slice(1));
		console.log(`Copied ${String(copied.length)} upstream files.`);
	} else if (command === 'adapt' && args.length) {
		await recordAdaptations(args);
		console.log(`Recorded ${String(args.length)} adaptations.`);
	} else if (command === 'reset' && args.length > 1) {
		await resetToUpstream(args[0], args.slice(1));
		console.log(`Reset ${String(args.length - 1)} files to upstream.`);
	} else {
		console.error(usage);
		process.exit(2);
	}
}

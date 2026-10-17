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

/** Copies `roots` and every upstream file they reach that the repository does not already contain. */
export async function importSources(upstreamDirectory, roots) {
	const upstream = new URL(`${upstreamDirectory.replace(/\/$/, '')}/`, 'file://');
	const commit = execFileSync('git', ['-C', upstream.pathname, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
	const manifest = JSON.parse(await readFile(manifestUrl, 'utf8'));
	if (commit !== manifest.commit) {
		throw new Error(`Upstream checkout is at ${commit}; the manifest pins ${manifest.commit}`);
	}
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
			manifest.files.push({ path: target, upstreamPath: path, sha256: createHash('sha256').update(flattened).digest('hex'), changedImports: 0 });
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
	manifest.files.sort((left, right) => left.path.localeCompare(right.path, 'en'));
	for (const path of roots.map(localPath)) {
		if (!manifest.entryPoints.includes(path)) {
			manifest.entryPoints.push(path);
		}
		if (!manifest.roots.includes(path)) {
			manifest.roots.push(path);
			manifest.roots.sort();
		}
	}
	const graph = await titlebarDependencies(manifest);
	if (graph.unresolved.length) {
		throw new Error(`Unresolved dependencies after import: ${JSON.stringify(graph.unresolved)}`);
	}
	manifest.dependencyEdges = graph.edges;
	manifest.externalDependencies = graph.external;
	await writeFile(manifestUrl, `${JSON.stringify(manifest, null, 2)}\n`);
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

if (process.argv[1] === new URL(import.meta.url).pathname) {
	const [upstream, ...roots] = process.argv.slice(2);
	if (!upstream || !roots.length) {
		console.error('Usage: node build/vscodeImport.mjs <vscode checkout at the pinned commit> <src/vs/...> [...]');
		process.exit(2);
	}
	const copied = await importSources(upstream, roots);
	console.log(`Copied ${String(copied.length)} upstream files.`);
}

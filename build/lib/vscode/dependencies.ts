// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The provenance graph: every copied file is reached from the application
 * through imports, and every copy matches its recorded checksum and, given an
 * upstream checkout, the text it is derived from.
 */
import ts from 'typescript';
import { readFile } from 'node:fs/promises';
import { join, posix } from 'node:path';
import { parseArgs } from 'node:util';
import { assertCheckout, copyKind, flattenSource, readManifest, sha256, type DependencyEdge, type ManifestFile, type ToolingFile, type VsCodeManifest } from './manifest.ts';
import { pruneSource } from './prune.ts';

export interface UnresolvedDependency {
	readonly from: string;
	readonly specifier: string;
	readonly kind: string;
}

export interface DependencyGraph {
	readonly roots: string[];
	readonly retained: Set<string>;
	readonly edges: DependencyEdge[];
	readonly external: string[];
	readonly unresolved: UnresolvedDependency[];
}

/** The specifiers `path` imports: module imports for scripts, `url()` and `@import` for CSS. */
export function sourceDependencies(path: string, content: string): { readonly specifier: string; readonly kind: string }[] {
	if (path.endsWith('.css')) {
		return [...content.matchAll(/url\(\s*['"]?([^'"\s)]+)['"]?\s*\)|@import\s+['"]([^'"]+)['"]/g)]
			.map(match => match[1] || match[2] || '')
			.filter(specifier => !/^(data:|https?:|#)/.test(specifier))
			.map(specifier => ({ specifier: specifier.startsWith('.') ? specifier : `./${specifier}`, kind: 'css asset' }));
	}
	if (!/\.[cm]?[jt]sx?$/.test(path)) {
		return [];
	}
	const found: { specifier: string; kind: string }[] = [];
	const ast = ts.createSourceFile(path, content, ts.ScriptTarget.Latest, true);
	const literal = (node: ts.Node | undefined, kind: string): void => {
		if (node && ts.isStringLiteralLike(node)) {
			found.push({ specifier: node.text, kind });
		}
	};
	const visit = (node: ts.Node): void => {
		if (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) {
			literal(node.moduleSpecifier, 'import');
		} else if (ts.isImportTypeNode(node) && ts.isLiteralTypeNode(node.argument)) {
			literal(node.argument.literal, 'type import');
		} else if (ts.isImportEqualsDeclaration(node) && ts.isExternalModuleReference(node.moduleReference)) {
			literal(node.moduleReference.expression, 'require');
		} else if (ts.isCallExpression(node)) {
			if (node.expression.kind === ts.SyntaxKind.ImportKeyword || (ts.isIdentifier(node.expression) && node.expression.text === 'require')) {
				literal(node.arguments[0], 'dynamic import');
			} else if (ts.isPropertyAccessExpression(node.expression) && ['asBrowserUri', 'asFileUri'].includes(node.expression.name.text)) {
				literal(node.arguments[0], 'asset');
			}
		} else if (ts.isNewExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 'URL') {
			literal(node.arguments?.[0], 'asset');
		}
		ts.forEachChild(node, visit);
	};
	visit(ast);
	for (const reference of ts.preProcessFile(content).referencedFiles) {
		found.push({ specifier: reference.fileName.startsWith('.') ? reference.fileName : `./${reference.fileName}`, kind: 'reference' });
	}
	return found;
}

/** The files a module path may name: `x.js` is `x.ts` in source, a bare path may be a directory index. */
export function moduleCandidates(path: string): string[] {
	const candidates = [path];
	if (/\.[cm]?js$/.test(path)) {
		candidates.push(path.replace(/\.js$/, '.ts'), path.replace(/\.js$/, '.tsx'), path.replace(/\.mjs$/, '.mts'), path.replace(/\.cjs$/, '.cts'), path.replace(/\.js$/, '.d.ts'));
	} else if (!posix.extname(path)) {
		candidates.push(`${path}.ts`, `${path}.js`, `${path}.d.ts`, `${path}/index.ts`);
	}
	return candidates;
}

/** Vendored JavaScript ships with its declarations and attribution. */
export function companionsOf(path: string): string[] {
	return path.endsWith('.js') ? [path.replace(/\.js$/, '.d.ts'), path.replace(/\.js$/, '.license.txt'), `${posix.dirname(path)}/cgmanifest.json`] : [];
}

/** Walks the copies from the manifest's roots and records how each is reached. */
export async function provenanceGraph(root: string, manifest: VsCodeManifest): Promise<DependencyGraph> {
	const available = new Set(manifest.files.map(file => file.path));
	const roots = [...manifest.entryPoints, ...manifest.compilerSupport];
	const application = new Set(manifest.applicationDependencies);
	const pending = [...roots];
	const retained = new Set<string>();
	const edges: DependencyEdge[] = [];
	const external = new Set<string>();
	const unresolved: UnresolvedDependency[] = [];
	const dependency = (from: string, specifier: string, kind: string): void => {
		if (kind === 'asset' && !specifier.startsWith('.') && !specifier.startsWith('vs/')) {
			return;
		}
		if (/^(data:|https?:|#)/.test(specifier)) {
			return;
		}
		let path: string;
		if (specifier.startsWith('.')) {
			path = posix.normalize(posix.join(posix.dirname(from), specifier.split(/[?#]/)[0]));
		} else if (specifier.startsWith('vs/')) {
			path = `src/${specifier.slice(3)}`;
		} else {
			external.add(specifier);
			return;
		}
		const target = moduleCandidates(path).find(candidate => available.has(candidate) || application.has(candidate));
		if (!target) {
			unresolved.push({ from, specifier, kind });
			return;
		}
		edges.push({ from, to: target, kind });
		if (!application.has(target)) {
			pending.push(target);
		}
	};
	for (let path = pending.pop(); path !== undefined; path = pending.pop()) {
		if (retained.has(path)) {
			continue;
		}
		if (!available.has(path)) {
			throw new Error(`Missing dependency root: ${path}`);
		}
		retained.add(path);
		for (const companion of companionsOf(path).filter(candidate => available.has(candidate))) {
			edges.push({ from: path, to: companion, kind: 'declaration or attribution' });
			pending.push(companion);
		}
		for (const { specifier, kind } of sourceDependencies(path, await readFile(join(root, path), 'utf8'))) {
			dependency(path, specifier, kind);
		}
	}
	return { roots: roots.sort(), retained, edges, external: [...external].sort(), unresolved };
}

/** Upstream's text for `file` after flattening, without the declarations it prunes. */
export async function derivedContent(file: ManifestFile | ToolingFile, checkout: string): Promise<string> {
	const upstream = await readFile(join(checkout, file.upstreamPath), 'utf8');
	const flattened = /\.(ts|js|css)$/.test(file.path) ? flattenSource(upstream) : upstream;
	const pruned = 'pruned' in file ? file.pruned ?? [] : [];
	return pruned.length ? pruneSource(file.path, flattened, pruned) : flattened;
}

/** The text a verbatim or pruned copy must equal; `undefined` for an adapted one, whose text is its own. */
export async function expectedContent(file: ManifestFile | ToolingFile, checkout: string): Promise<string | undefined> {
	return copyKind(file) === 'adapted' ? undefined : derivedContent(file, checkout);
}

/**
 * The `check:upstream` task: every copy matches its checksum and the graph.
 * With `--checkout <dir>` (VS Code at the pinned commit), verbatim and pruned
 * copies are also re-derived from upstream and compared byte for byte.
 */
export async function verifyUpstream(root: string, args: readonly string[] = []): Promise<void> {
	const { values } = parseArgs({ args: [...args], options: { checkout: { type: 'string' } }, strict: true });
	const manifest = await readManifest(root);
	const changed: string[] = [];
	for (const file of [...manifest.files, ...manifest.tooling]) {
		if (sha256(await readFile(join(root, file.path))) !== file.sha256) {
			changed.push(file.path);
		}
	}
	if (changed.length) {
		throw new Error(`Copied VS Code sources changed without being recorded:\n${changed.join('\n')}\nRecord an edit with \`vscode adapt\`, a deletion with \`vscode prune\`, or restore it with \`vscode reset\`.`);
	}
	const graph = await provenanceGraph(root, manifest);
	if (graph.unresolved.length) {
		throw new Error(`Copied sources import files that are neither copies nor application modules: ${JSON.stringify(graph.unresolved)}`);
	}
	const unreached = manifest.files.map(file => file.path).filter(path => !graph.retained.has(path));
	if (unreached.length) {
		throw new Error(`Copies that nothing imports (remove them with \`vscode remove\`):\n${unreached.join('\n')}`);
	}
	if (JSON.stringify(graph.roots) !== JSON.stringify([...manifest.roots].sort())) {
		throw new Error('The provenance graph roots differ from the manifest; run `node build/run.ts vscode sync`.');
	}
	let compared = 0;
	if (values.checkout) {
		await assertCheckout(manifest, values.checkout);
		const different: string[] = [];
		for (const file of [...manifest.files, ...manifest.tooling]) {
			const expected = await expectedContent(file, values.checkout);
			if (expected === undefined) {
				continue;
			}
			compared++;
			if (expected !== await readFile(join(root, file.path), 'utf8')) {
				different.push(`${file.path} (${copyKind(file)})`);
			}
		}
		if (different.length) {
			throw new Error(`Copies that differ from upstream ${manifest.ref} beyond their recorded kind:\n${different.join('\n')}`);
		}
	}
	console.log(`VS Code ${manifest.ref}: ${String(manifest.files.length + manifest.tooling.length)} copied files verified${values.checkout ? `, ${String(compared)} compared with upstream` : ''}.`);
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { createHash } from 'node:crypto';
import ts from 'typescript';
import { readFile } from 'node:fs/promises';
import { posix } from 'node:path';

const root = new URL('../', import.meta.url);

export interface ManifestFile {
	path: string;
	upstreamPath: string;
	sha256: string;
	changedImports?: number;
	adapted?: boolean;
	upstreamStyle?: boolean;
}

export interface DependencyEdge {
	readonly from: string;
	readonly to: string;
	readonly kind: string;
}

interface UnresolvedDependency {
	readonly from: string;
	readonly specifier: string;
	readonly kind: string;
}

/** `build/vscode-source.json`: the pinned upstream commit and every copied file's provenance. */
export interface VsCodeManifest {
	ref: string;
	commit: string;
	files: ManifestFile[];
	roots: string[];
	entryPoints: string[];
	compilerSupport?: string[];
	applicationDependencies: string[];
	dependencyEdges: DependencyEdge[];
	externalDependencies: string[];
}

export interface DependencyGraph {
	readonly roots: string[];
	readonly retained: Set<string>;
	readonly edges: DependencyEdge[];
	readonly external: string[];
	readonly unresolved: UnresolvedDependency[];
}

export async function titlebarDependencies(manifest: VsCodeManifest): Promise<DependencyGraph> {
	const available = new Set(manifest.files.map(file => file.path));
	const roots = [...manifest.entryPoints, ...(manifest.compilerSupport ?? [])];
	const application = new Set(manifest.applicationDependencies);
	const pending = [...roots];
	const retained = new Set<string>();
	const edges: DependencyEdge[] = [];
	const external = new Set<string>();
	const unresolved: UnresolvedDependency[] = [];
	function dependency(from: string, specifier: string, kind = 'import'): void {
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
		const candidates = [path];
		if (/\.[cm]?js$/.test(path)) {
			candidates.push(path.replace(/\.js$/, '.ts'), path.replace(/\.js$/, '.tsx'), path.replace(/\.mjs$/, '.mts'), path.replace(/\.cjs$/, '.cts'), path.replace(/\.js$/, '.d.ts'));
		} else if (!posix.extname(path)) {
			candidates.push(`${path}.ts`, `${path}.js`, `${path}.d.ts`, `${path}/index.ts`);
		}
		const target = candidates.find(candidate => available.has(candidate) || application.has(candidate));
		if (!target) {
			unresolved.push({ from, specifier, kind });
			return;
		}
		edges.push({ from, to: target, kind });
		if (!application.has(target)) {
			pending.push(target);
		}
	}
	while (pending.length) {
		const next = pending.pop();
		if (next === undefined || retained.has(next)) {
			continue;
		}
		const path = next;
		if (!available.has(path)) {
			throw new Error(`Missing dependency root: ${path}`);
		}
		retained.add(path);
		if (path.endsWith('.js')) {
			for (const companion of [path.replace(/\.js$/, '.d.ts'), path.replace(/\.js$/, '.license.txt'), `${posix.dirname(path)}/cgmanifest.json`]) {
				if (available.has(companion)) {
					edges.push({ from: path, to: companion, kind: 'declaration or attribution' });
					pending.push(companion);
				}
			}
		}
		const content = await readFile(new URL(path, root), 'utf8');
		if (/\.[cm]?[jt]sx?$/.test(path)) {
			const ast = ts.createSourceFile(path, content, ts.ScriptTarget.Latest, true);
			function literal(node: ts.Node | undefined, kind: string): void {
				if (node && ts.isStringLiteralLike(node)) {
					dependency(path, node.text, kind);
				}
			}
			function visit(node: ts.Node): void {
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
			}
			visit(ast);
			for (const reference of ts.preProcessFile(content).referencedFiles) {
				dependency(path, reference.fileName.startsWith('.') ? reference.fileName : `./${reference.fileName}`, 'reference');
			}
		} else if (path.endsWith('.css')) {
			for (const match of content.matchAll(/url\(\s*['"]?([^'"\s)]+)['"]?\s*\)|@import\s+['"]([^'"]+)['"]/g)) {
				const specifier = match[1] || match[2] || '';
				dependency(path, specifier.startsWith('.') || /^(data:|https?:|#)/.test(specifier) ? specifier : `./${specifier}`, 'css asset');
			}
		}
	}
	return { roots: roots.sort(), retained, edges, external: [...external].sort(), unresolved };
}

/** Checks every copied file against its recorded checksum and the provenance graph. */
export async function verifyUpstream(): Promise<void> {
	const manifest = JSON.parse(await readFile(new URL('./vscode-source.json', import.meta.url), 'utf8')) as VsCodeManifest;
	for (const file of manifest.files) {
		const content = await readFile(new URL(file.path, root));
		if (createHash('sha256').update(content).digest('hex') !== file.sha256) {
			throw new Error(`Imported VS Code source changed: ${file.path}`);
		}
	}
	const graph = await titlebarDependencies(manifest);
	if (graph.unresolved.length) {
		throw new Error(`Missing titlebar dependencies: ${JSON.stringify(graph.unresolved)}`);
	}
	if (graph.retained.size !== manifest.files.length) {
		throw new Error('Imported source contains files outside the titlebar dependency graph.');
	}
	if (JSON.stringify(graph.roots) !== JSON.stringify(manifest.roots)) {
		throw new Error('Titlebar dependency roots differ from the import manifest.');
	}
	console.log(`VS Code ${manifest.ref}: ${String(manifest.files.length)} imported source files verified.`);
}

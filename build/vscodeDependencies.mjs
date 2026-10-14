import ts from 'typescript';
import { readFile } from 'node:fs/promises';
import { posix } from 'node:path';
const root = new URL('../', import.meta.url);
export async function titlebarDependencies(manifest) {
	const available = new Set(manifest.files.map(file => file.path));
	const roots = [...manifest.entryPoints, ...(manifest.compilerSupport || [])];
	const application = new Set(manifest.applicationDependencies || []);
	const pending = [...roots];
	const retained = new Set();
	const edges = [];
	const external = new Set();
	const unresolved = [];
	function dependency(from, specifier, kind = 'import') {
		if (kind === 'asset' && !specifier.startsWith('.') && !specifier.startsWith('vs/')) {
			return;
		}
		if (/^(data:|https?:|#)/.test(specifier)) {
			return;
		}
		let path;
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
		const path = pending.pop();
		if (retained.has(path)) {
			continue;
		}
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
			function literal(node, kind) {
				if (node && ts.isStringLiteralLike(node)) {
					dependency(path, node.text, kind);
				}
			}
			function visit(node) {
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
				const specifier = match[1] || match[2];
				dependency(path, specifier.startsWith('.') || /^(data:|https?:|#)/.test(specifier) ? specifier : `./${specifier}`, 'css asset');
			}
		}
	}
	return { roots: roots.sort(), retained, edges, external: [...external].sort(), unresolved };
}

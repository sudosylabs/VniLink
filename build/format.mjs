import ts from 'typescript';
import prettier from 'prettier';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { resolve, extname, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isApplicationFile } from './sourceScope.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));

const settings = JSON.parse(await readFile(new URL('../tsfmt.json', import.meta.url), 'utf8'));

function expandStructure(filename, text) {
	const source = ts.createSourceFile(filename, text, ts.ScriptTarget.Latest, true);
	const changes = new Map();
	const gap = (start, end, lines) => {
		if (/^\s*$/.test(text.slice(start, end))) {
			changes.set(`${start}:${end}`, { start, end, value: typeof lines === 'string' ? lines : '\n'.repeat(lines) });
		}
	};
	const visit = node => {
		if (ts.isTryStatement(node) || ts.isIfStatement(node)) {
			const children = node.getChildren(source);
			for (let index = 1; index < children.length; index++) {
				const current = children[index];
				if (ts.isCatchClause(current) || current.kind === ts.SyntaxKind.FinallyKeyword || current.kind === ts.SyntaxKind.ElseKeyword) {
					const previous = children[index - 1];
					if (ts.isBlock(previous) || ts.isCatchClause(previous)) {
						gap(previous.end, current.getStart(source), ' ');
					}
				}
			}
		}
		if (ts.isSourceFile(node) || ts.isBlock(node)) {
			const statements = node.statements;
			for (let index = 1; index < statements.length; index++) {
				const previous = statements[index - 1];
				const current = statements[index];
				const lines = ts.isSourceFile(node) && !(ts.isImportDeclaration(previous) && ts.isImportDeclaration(current)) ? 2 : 1;
				gap(previous.end, current.getStart(source), lines);
			}
			if (ts.isBlock(node) && statements.length) {
				gap(node.getStart(source) + 1, statements[0].getStart(source), 1);
				gap(statements.at(-1).end, node.end - 1, 1);
			}
		}
		if (ts.isClassDeclaration(node) || ts.isClassExpression(node)) {
			for (let index = 1; index < node.members.length; index++) {
				const previous = node.members[index - 1];
				const current = node.members[index];
				const lines = ts.isPropertyDeclaration(previous) && ts.isPropertyDeclaration(current) ? 1 : 2;
				gap(previous.end, current.getStart(source), lines);
			}
		}
		ts.forEachChild(node, visit);
	};
	visit(source);
	for (const change of [...changes.values()].sort((a, b) => b.start - a.start)) {
		text = text.slice(0, change.start) + change.value + text.slice(change.end);
	}
	return text;
}

export function formatTypeScript(filename, text) {
	text = expandStructure(filename, text);
	const host = {
		getScriptFileNames: () => [filename], getScriptVersion: () => '0',
		getScriptSnapshot: name => name === filename ? ts.ScriptSnapshot.fromString(text) : undefined,
		getCurrentDirectory: () => root, getCompilationSettings: () => ({ allowJs: true }),
		getDefaultLibFileName: options => ts.getDefaultLibFilePath(options),
		fileExists: name => name === filename, readFile: name => name === filename ? text : undefined,
	};
	const service = ts.createLanguageService(host);
	try {
		const edits = service.getFormattingEditsForDocument(filename, { ...settings, newLineCharacter: '\n' });
		for (const edit of edits.sort((a, b) => b.span.start - a.span.start)) {
			text = text.slice(0, edit.span.start) + edit.newText + text.slice(edit.span.start + edit.span.length);
		}
		return text.endsWith('\n') ? text : text + '\n';
	} finally {
		service.dispose();
	}
}

async function collect(directory) {
	const files = [];
	for (const entry of await readdir(directory, { withFileTypes: true })) {
		const path = resolve(directory, entry.name);
		if (entry.isDirectory()) {
			files.push(...await collect(path));
		} else if (/\.(ts|mjs|css|html|json)$/.test(entry.name) && isApplicationFile(path)) {
			files.push(path);
		}
	}
	return files;
}

async function run() {
	const write = process.argv.includes('--write');
	const roots = ['src', 'build', 'scripts', 'test', '.vscode'];
	let files = (await Promise.all(roots.map(path => collect(resolve(root, path))))).flat();
	files.push(...['package.json', 'tsconfig.json', 'tsfmt.json', '.prettierrc.json', 'eslint.config.mjs', 'stylelint.config.mjs', '.lintstagedrc.mjs'].map(path => resolve(root, path)));
	const selected = process.argv.indexOf('--files');
	if (selected !== -1) {
		const allowed = new Set(files);
		files = process.argv.slice(selected + 1).map(filename => resolve(filename));
		for (const filename of files) {
			if (!allowed.has(filename)) {
				throw new Error(`File outside formatting scope: ${relative(root, filename)}`);
			}
		}
	}
	let changed = 0;
	for (const filename of files.sort()) {
		const text = await readFile(filename, 'utf8');
		const formatted = ['.ts', '.mjs'].includes(extname(filename)) ? formatTypeScript(filename, text) : await prettier.format(text, { ...(await prettier.resolveConfig(filename)), filepath: filename });
		if (text !== formatted) {
			changed++;
			if (write) {
				await writeFile(filename, formatted);
			}
			console.log(`${write ? 'Formatted' : 'Needs formatting'}: ${filename.slice(root.length)}`);
		}
	}
	if (!write && changed) {
		process.exitCode = 1;
	}
	console.log(`Formatting: ${files.length} application/tooling files checked; ${changed} ${write ? 'updated' : 'differ'}.`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	await run();
}

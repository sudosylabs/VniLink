import ts from 'typescript';
import prettier from 'prettier';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { resolve, extname } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const settings = JSON.parse(await readFile(new URL('../tsfmt.json', import.meta.url), 'utf8'));
export function formatTypeScript(filename, text) {
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
		for (const edit of edits.sort((a, b) => b.span.start - a.span.start)) { text = text.slice(0, edit.span.start) + edit.newText + text.slice(edit.span.start + edit.span.length); }
		return text.endsWith('\n') ? text : text + '\n';
	} finally { service.dispose(); }
}
async function collect(directory) {
	const files = [];
	for (const entry of await readdir(directory, { withFileTypes: true })) {
		const path = resolve(directory, entry.name);
		if (entry.isDirectory()) { files.push(...await collect(path)); }
		else if (/\.(ts|mjs|css|html|json)$/.test(entry.name)) { files.push(path); }
	}
	return files;
}
async function run() {
	const write = process.argv.includes('--write');
	const roots = ['src', 'build', 'scripts', 'test', '.vscode'];
	const files = (await Promise.all(roots.map(path => collect(resolve(root, path))))).flat();
	files.push(...['package.json', 'tsconfig.json', 'tsfmt.json', '.prettierrc.json', 'eslint.config.mjs', 'stylelint.config.mjs'].map(path => resolve(root, path)));
	let changed = 0;
	for (const filename of files.sort()) {
		const text = await readFile(filename, 'utf8');
		const formatted = ['.ts', '.mjs'].includes(extname(filename)) ? formatTypeScript(filename, text) : await prettier.format(text, { ...(await prettier.resolveConfig(filename)), filepath: filename });
		if (text !== formatted) {
			changed++;
			if (write) { await writeFile(filename, formatted); }
			console.log(`${write ? 'Formatted' : 'Needs formatting'}: ${filename.slice(root.length)}`);
		}
	}
	if (!write && changed) { process.exitCode = 1; }
	console.log(`Formatting: ${files.length} application/tooling files checked; ${changed} ${write ? 'updated' : 'differ'}.`);
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) { await run(); }

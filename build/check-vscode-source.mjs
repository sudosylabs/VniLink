import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { titlebarDependencies } from './vscodeDependencies.mjs';

const root = new URL('../', import.meta.url);

const manifest = JSON.parse(await readFile(new URL('./vscode-source.json', import.meta.url), 'utf8'));

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

console.log(`VS Code ${manifest.ref}: ${manifest.files.length} imported source files verified.`);

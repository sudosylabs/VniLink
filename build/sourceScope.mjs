import { readFileSync, readdirSync } from 'node:fs';
import { posix, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const manifest = JSON.parse(readFileSync(new URL('./vscode-source.json', import.meta.url), 'utf8'));
export const upstreamFiles = manifest.files.filter(file => !file.adapted).map(file => file.path);
const upstream = new Set(upstreamFiles);
const applicationDirectories = new Set();
function findApplicationDirectories(directory) {
	for (const entry of readdirSync(resolve(root, directory), { withFileTypes: true })) {
		const path = `${directory}/${entry.name}`;
		if (entry.isDirectory()) {
			findApplicationDirectories(path);
		} else if (!upstream.has(path)) {
			let parent = posix.dirname(path);
			while (parent !== '.') {
				applicationDirectories.add(parent);
				parent = posix.dirname(parent);
			}
		}
	}
}
findApplicationDirectories('src');
export const upstreamIgnores = [...new Set(upstreamFiles.map(path => {
	const segments = path.split('/');
	for (let index = 1; index < segments.length; index++) {
		const directory = segments.slice(0, index).join('/');
		if (!applicationDirectories.has(directory)) {
			return `${directory}/**`;
		}
	}
	return path;
}))];
export function isApplicationFile(filename) {
	const path = relative(root, resolve(filename)).split(sep).join('/');
	return !upstream.has(path) && path !== 'build/vscode-source.json';
}

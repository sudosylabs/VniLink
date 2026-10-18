import { readFileSync, readdirSync } from 'node:fs';
import { posix, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));

const manifest = JSON.parse(readFileSync(new URL('./vscode-source.json', import.meta.url), 'utf8')) as { files: { path: string; adapted?: boolean; upstreamStyle?: boolean }[] };

// Adapted infrastructure marked `upstreamStyle` keeps VS Code's formatting and lint baseline;
// only its declared edits differ. Other adaptations are VniLink code held to application rules.
export const upstreamFiles = manifest.files.filter(file => !file.adapted || file.upstreamStyle === true).map(file => file.path);

const upstream = new Set(upstreamFiles);

const applicationDirectories = new Set<string>();

function findApplicationDirectories(directory: string): void {
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

// Verbatim VS Code tooling copies (ESLint rules) keep upstream formatting.
const upstreamToolingDirectories = ['build/lint/vscode/'];

export function isApplicationFile(filename: string): boolean {
	const path = relative(root, resolve(filename)).split(sep).join('/');
	return !upstream.has(path) && path !== 'build/vscode-source.json' && !upstreamToolingDirectories.some(directory => path.startsWith(directory));
}

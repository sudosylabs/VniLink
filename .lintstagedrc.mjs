import { isApplicationFile } from './build/sourceScope.mjs';
function scoped(commands) {
	return files => {
		const selected = files.filter(isApplicationFile);
		if (!selected.length) {
			return [];
		}
		const argumentsList = selected.map(file => JSON.stringify(file)).join(' ');
		return commands.map(command => `${command} ${argumentsList}`);
	};
}
export default {
	'{src,test}/**/*.ts': scoped(['eslint --fix --max-warnings 0', 'node build/format.mjs --write --files']),
	'{build,scripts,test}/**/*.mjs': scoped(['eslint --fix --max-warnings 0', 'node build/format.mjs --write --files']),
	'{*.config.mjs,.lintstagedrc.mjs}': scoped(['eslint --fix --max-warnings 0', 'node build/format.mjs --write --files']),
	'src/**/*.css': scoped(['stylelint --fix --max-warnings 0', 'node build/format.mjs --write --files']),
	'{src,build,scripts,test,.vscode}/**/*.{json,html}': scoped(['node build/format.mjs --write --files']),
	'{package.json,tsconfig.json,tsfmt.json,.prettierrc.json}': scoped(['node build/format.mjs --write --files']),
};

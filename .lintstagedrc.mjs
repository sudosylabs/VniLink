import { isApplicationFile } from './build/sourceScope.ts';

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
	'{src,test,build}/**/*.ts': scoped(['eslint --fix --max-warnings 0', 'node build/run.ts format --write --files']),
	'test/**/*.mjs': scoped(['eslint --fix --max-warnings 0', 'node build/run.ts format --write --files']),
	'{*.config.mjs,.lintstagedrc.mjs}': scoped(['eslint --fix --max-warnings 0', 'node build/run.ts format --write --files']),
	'src/**/*.css': scoped(['stylelint --fix --max-warnings 0', 'node build/run.ts format --write --files']),
	'{src,build,scripts,test,.vscode}/**/*.{json,html}': scoped(['node build/run.ts format --write --files']),
	'{package.json,tsconfig.json,tsfmt.json,.prettierrc.json}': scoped(['node build/run.ts format --write --files']),
};

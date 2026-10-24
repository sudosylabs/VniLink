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

// Types and Rust formatting span files: one project-wide check when any such file is staged.
function typechecked(fix) {
	return files => [...fix(files), 'npm run typecheck'];
}

const rustfmt = () => 'cargo fmt --all -- --check';

export default {
	'{src,test,build}/**/*.ts': typechecked(scoped(['eslint --fix --max-warnings 0', 'node build/run.ts format --write --files'])),
	'test/**/*.mjs': scoped(['eslint --fix --max-warnings 0', 'node build/run.ts format --write --files']),
	'{*.config.mjs,.lintstagedrc.mjs}': scoped(['eslint --fix --max-warnings 0', 'node build/run.ts format --write --files']),
	'src/**/*.css': scoped(['stylelint --fix --max-warnings 0', 'node build/run.ts format --write --files']),
	'{src,build,scripts,test,.vscode}/**/*.{json,html}': scoped(['node build/run.ts format --write --files']),
	'{package.json,tsconfig.json,tsfmt.json,.prettierrc.json}': scoped(['node build/run.ts format --write --files']),
	'crates/**/*.rs': rustfmt,
};

export default {
	'{src,test}/**/*.ts': ['eslint --fix --max-warnings 0', 'node build/format.mjs --write --files'],
	'{build,scripts,test}/**/*.mjs': ['eslint --fix --max-warnings 0', 'node build/format.mjs --write --files'],
	'{*.config.mjs,.lintstagedrc.mjs}': ['eslint --fix --max-warnings 0', 'node build/format.mjs --write --files'],
	'src/**/*.css': ['stylelint --fix --max-warnings 0', 'node build/format.mjs --write --files'],
	'{src,build,scripts,test,.vscode}/**/*.{json,html}': 'node build/format.mjs --write --files',
	'{package.json,tsconfig.json,tsfmt.json,.prettierrc.json}': 'node build/format.mjs --write --files',
};

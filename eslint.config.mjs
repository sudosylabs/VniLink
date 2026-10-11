import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import stylistic from '@stylistic/eslint-plugin';
import globals from 'globals';
import layers from './build/lint/layers.mjs';
export default [
	{ ignores: ['node_modules/**', 'out/**', 'dist/**', 'target/**', 'vendor/**', 'crates/**', 'docs/**', 'assets/**'] },
	{ ...js.configs.recommended, files: ['**/*.mjs'] },
	...tseslint.configs.recommendedTypeChecked.map(config => ({ ...config, files: ['src/**/*.ts', 'test/**/*.ts'] })),
	{
		files: ['**/*.mjs', 'src/**/*.ts', 'test/**/*.ts'],
		languageOptions: { globals: globals.node },
		plugins: { '@stylistic': stylistic, vnilink: layers },
		rules: {
			curly: ['error', 'all'], eqeqeq: ['error', 'always'], 'prefer-const': 'error', 'no-var': 'error',
			'no-eval': 'error', 'no-new-func': 'error', 'no-duplicate-imports': ['error', { allowSeparateTypeImports: true }],
			'@stylistic/semi': ['error', 'always'], '@stylistic/quotes': ['error', 'single', { avoidEscape: true, allowTemplateLiterals: 'always' }],
			'vnilink/layers': 'error',
		},
	},
	{
		files: ['src/**/*.ts', 'test/**/*.ts'],
		languageOptions: { parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname } },
		rules: {
			'@typescript-eslint/consistent-type-imports': ['error', { prefer: 'type-imports' }],
			'@typescript-eslint/no-floating-promises': 'error', '@typescript-eslint/no-misused-promises': 'error',
		},
	},
	{
		files: ['src/renderer.ts', 'src/**/browser/**/*.ts'],
		languageOptions: { globals: { ...Object.fromEntries(Object.keys(globals.node).map(name => [name, 'off'])), ...globals.browser } },
		rules: { 'no-restricted-globals': ['error', 'process', 'Buffer', 'require', '__dirname', '__filename'] },
	},
	{
		files: ['src/**/common/**/*.ts'],
		rules: { 'no-restricted-globals': ['error', 'window', 'document', 'navigator', 'process', 'Buffer'] },
	},
];

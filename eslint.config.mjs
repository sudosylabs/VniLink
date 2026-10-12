import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import stylistic from '@stylistic/eslint-plugin';
import globals from 'globals';
import layers from './build/lint/layers.mjs';
import imports from './build/lint/imports.mjs';
import synchronousFs from './build/lint/synchronousFs.mjs';
export default [
	{ ignores: ['node_modules/**', 'out/**', 'dist/**', 'target/**', 'vendor/**', 'crates/**', 'docs/**', 'assets/**'] },
	{ ...js.configs.recommended, files: ['**/*.mjs'] },
	...[...tseslint.configs.strictTypeChecked, ...tseslint.configs.stylisticTypeChecked].map(config => ({ ...config, files: ['src/**/*.ts', 'test/**/*.ts'] })),
	{
		files: ['**/*.mjs', 'src/**/*.ts', 'test/**/*.ts'],
		languageOptions: { globals: globals.node },
		plugins: { '@stylistic': stylistic, vnilink: { rules: { ...layers.rules, ...imports.rules, ...synchronousFs.rules } } },
		rules: {
			curly: ['error', 'all'], eqeqeq: ['error', 'always'], 'prefer-const': 'error', 'no-var': 'error',
			'no-eval': 'error', 'no-new-func': 'error', 'no-duplicate-imports': ['error', { allowSeparateTypeImports: true }],
			'@stylistic/semi': ['error', 'always'], '@stylistic/quotes': ['error', 'single', { avoidEscape: true, allowTemplateLiterals: 'always' }],
			'vnilink/layers': 'error',
			'no-new-wrappers': 'error',
			'@stylistic/brace-style': ['error', '1tbs'],
		},
	},
	{
		files: ['src/**/*.ts', 'test/**/*.ts'],
		languageOptions: { parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname } },
		rules: {
			'vnilink/import-extensions': 'error',
			'no-restricted-syntax': ['error', { selector: 'TSAsExpression > TSAsExpression', message: 'Avoid chained type assertions; validate unknown data with a type guard.' }],
			'@typescript-eslint/consistent-type-assertions': ['error', { assertionStyle: 'as', objectLiteralTypeAssertions: 'never', arrayLiteralTypeAssertions: 'never' }],
			'@typescript-eslint/switch-exhaustiveness-check': 'error',
			'@typescript-eslint/explicit-member-accessibility': ['error', { accessibility: 'explicit', overrides: { constructors: 'no-public' } }],
			'@typescript-eslint/consistent-type-imports': ['error', { prefer: 'type-imports' }],
			'@typescript-eslint/no-floating-promises': 'error', '@typescript-eslint/no-misused-promises': 'error',
		},
	},
	{ files: ['src/**/*.ts'], rules: { 'vnilink/no-sync-fs': 'error' } },
	{
		files: ['src/renderer.ts', 'src/**/browser/**/*.ts'],
		languageOptions: { globals: { ...Object.fromEntries(Object.keys(globals.node).map(name => [name, 'off'])), ...globals.browser } },
		rules: { 'no-restricted-globals': ['error', 'process', 'Buffer', 'require', '__dirname', '__filename', 'module', 'exports', 'global', 'setImmediate', 'clearImmediate'] },
	},
	{
		files: ['src/**/common/**/*.ts'],
		languageOptions: { globals: Object.fromEntries(Object.keys(globals.node).map(name => [name, 'off'])) },
		rules: { 'no-restricted-globals': ['error', 'window', 'document', 'navigator', 'process', 'Buffer', 'require', '__dirname', '__filename', 'module', 'exports', 'global', 'setImmediate', 'clearImmediate'] },
	},
];

import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import stylistic from '@stylistic/eslint-plugin';
import globals from 'globals';
import layers from './build/lint/layers.mjs';
import imports from './build/lint/imports.mjs';
import synchronousFs from './build/lint/synchronousFs.mjs';
import workbenchDom from './build/lint/workbenchDom.mjs';
import { upstreamIgnores } from './build/sourceScope.mjs';
import mustUseSuperDispose from './build/lint/vscode/code-must-use-super-dispose.ts';
import declareServiceBrand from './build/lint/vscode/code-declare-service-brand.ts';
import noDangerousTypeAssertions from './build/lint/vscode/code-no-dangerous-type-assertions.ts';
import noPotentiallyUnsafeDisposables from './build/lint/vscode/code-no-potentially-unsafe-disposables.ts';
import parameterPropertiesAccessibility from './build/lint/vscode/code-parameter-properties-must-have-explicit-accessibility.ts';
import noAccessorAfterAwait from './build/lint/vscode/code-no-accessor-after-await.ts';
import noLocalizationTemplateLiterals from './build/lint/vscode/code-no-localization-template-literals.ts';

// Rules ported verbatim from VS Code's `.eslint-plugin-local`; see build/lint/vscode.
const vscodeRules = {
	'code-must-use-super-dispose': mustUseSuperDispose,
	'code-declare-service-brand': declareServiceBrand,
	'code-no-dangerous-type-assertions': noDangerousTypeAssertions,
	'code-no-potentially-unsafe-disposables': noPotentiallyUnsafeDisposables,
	'code-parameter-properties-must-have-explicit-accessibility': parameterPropertiesAccessibility,
	'code-no-accessor-after-await': noAccessorAfterAwait,
	'code-no-localization-template-literals': noLocalizationTemplateLiterals,
};

export default [
	{ ignores: ['node_modules/**', 'out/**', 'dist/**', 'target/**', 'vendor/**', 'crates/**', 'docs/**', 'assets/**', ...upstreamIgnores] },
	{ ...js.configs.recommended, files: ['**/*.mjs'] },
	...[...tseslint.configs.strictTypeChecked, ...tseslint.configs.stylisticTypeChecked].map(config => ({ ...config, files: ['src/**/*.ts', 'test/**/*.ts'] })),
	{
		files: ['**/*.mjs', 'src/**/*.ts', 'test/**/*.ts'],
		languageOptions: { globals: globals.node },
		plugins: { '@stylistic': stylistic, vnilink: { rules: { ...layers.rules, ...imports.rules, ...synchronousFs.rules, ...workbenchDom.rules } } },
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
		languageOptions: { parserOptions: { project: './tsconfig.json', tsconfigRootDir: import.meta.dirname } },
		rules: {
			'vnilink/import-extensions': 'error',
			'no-restricted-syntax': ['error', { selector: 'TSAsExpression > TSAsExpression', message: 'Avoid chained type assertions; validate unknown data with a type guard.' }],
			'@typescript-eslint/consistent-type-assertions': ['error', { assertionStyle: 'as', objectLiteralTypeAssertions: 'never', arrayLiteralTypeAssertions: 'never' }],
			'@typescript-eslint/switch-exhaustiveness-check': 'error',
			'@typescript-eslint/explicit-member-accessibility': ['error', { accessibility: 'explicit', overrides: { constructors: 'no-public' } }],
			'@typescript-eslint/consistent-type-imports': ['error', { prefer: 'type-imports' }],
			'@typescript-eslint/no-floating-promises': 'error',
			// VS Code actions override `Action2.run(): void` with promise-returning implementations.
			'@typescript-eslint/no-misused-promises': ['error', { checksVoidReturn: { inheritedMethods: false } }],
		},
	},
	{ files: ['src/**/*.ts'], rules: { 'vnilink/no-sync-fs': 'error' } },
	{
		files: ['src/workbench/**/*.ts'],
		rules: { 'vnilink/workbench-dom': 'error' },
	},
	{
		files: ['src/**/*.ts', 'test/**/*.ts'],
		plugins: { local: { rules: vscodeRules } },
		rules: Object.fromEntries(Object.keys(vscodeRules).map(name => [`local/${name}`, 'error'])),
	},
	{
		// Upstream-faithful adaptive copy: the async contract methods are
		// synchronous in implementation and the CLI switch is compared
		// against its enum, matching VS Code.
		files: ['src/platform/encryption/electron-main/encryptionMainService.ts'],
		rules: { '@typescript-eslint/require-await': 'off', '@typescript-eslint/no-unsafe-enum-assignment': 'off', '@typescript-eslint/no-unsafe-enum-comparison': 'off' },
	},
	{
		files: ['src/renderer.ts', 'src/**/browser/**/*.ts', 'src/workbench/**/*.ts'],
		languageOptions: { globals: { ...Object.fromEntries(Object.keys(globals.node).map(name => [name, 'off'])), ...globals.browser } },
		rules: { 'no-restricted-globals': ['error', 'process', 'Buffer', 'require', '__dirname', '__filename', 'module', 'exports', 'global', 'setImmediate', 'clearImmediate'] },
	},
	{
		files: ['src/**/common/**/*.ts'],
		languageOptions: { globals: Object.fromEntries(Object.keys(globals.node).map(name => [name, 'off'])) },
		rules: { 'no-restricted-globals': ['error', 'window', 'document', 'navigator', 'process', 'Buffer', 'require', '__dirname', '__filename', 'module', 'exports', 'global', 'setImmediate', 'clearImmediate'] },
	},
];

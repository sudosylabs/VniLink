// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import stylistic from '@stylistic/eslint-plugin';
import globals from 'globals';
import layers from './build/lint/layers.ts';
import imports from './build/lint/imports.ts';
import synchronousFs from './build/lint/synchronousFs.ts';
import workbenchDom from './build/lint/workbenchDom.ts';
import buildClasses from './build/lint/buildClasses.ts';
import { upstreamIgnores } from './build/sourceScope.ts';
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
	{ ignores: ['node_modules/**', 'out/**', 'dist/**', 'target/**', 'vendor/**', 'crates/**', 'docs/**', 'assets/**', 'build/lint/vscode/**', '.claude/**', ...upstreamIgnores] },
	{ ...js.configs.recommended, files: ['**/*.mjs'] },
	...[...tseslint.configs.strictTypeChecked, ...tseslint.configs.stylisticTypeChecked].map(config => ({ ...config, files: ['src/**/*.ts', 'test/**/*.ts', 'build/**/*.ts'] })),
	{
		files: ['**/*.mjs', 'src/**/*.ts', 'test/**/*.ts', 'build/**/*.ts'],
		languageOptions: { globals: globals.node },
		plugins: { '@stylistic': stylistic, vnilink: { rules: { ...layers.rules, ...imports.rules, ...synchronousFs.rules, ...workbenchDom.rules, ...buildClasses.rules } } },
		rules: {
			curly: ['error', 'all'], eqeqeq: ['error', 'always'], 'prefer-const': 'error', 'no-var': 'error',
			'no-eval': 'error', 'no-new-func': 'error', 'no-duplicate-imports': ['error', { allowSeparateTypeImports: true }],
			'@stylistic/semi': ['error', 'always'], '@stylistic/quotes': ['error', 'single', { avoidEscape: true, allowTemplateLiterals: 'always' }],
			'vnilink/layers': 'error',
			'no-new-wrappers': 'error',
			'@stylistic/brace-style': ['error', '1tbs'],
			// Long strings, URLs and patterns stay whole; code wraps before 160 columns.
			'@stylistic/max-len': ['error', { code: 160, tabWidth: 4, ignoreStrings: true, ignoreUrls: true, ignoreTemplateLiterals: true, ignoreRegExpLiterals: true }],
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
	{
		// Build code runs under Node's type stripping: relative imports name their `.ts` file.
		files: ['build/**/*.ts'],
		languageOptions: { parserOptions: { project: './build/tsconfig.json', tsconfigRootDir: import.meta.dirname } },
		rules: {
			'@typescript-eslint/consistent-type-imports': ['error', { prefer: 'type-imports' }],
			'@typescript-eslint/explicit-member-accessibility': ['error', { accessibility: 'explicit', overrides: { constructors: 'no-public' } }],
			'@typescript-eslint/no-floating-promises': 'error',
			'@typescript-eslint/switch-exhaustiveness-check': 'error',
			'vnilink/build-classes': ['error', { allow: ['build/lib/processes.ts', 'build/lib/watch.ts', 'build/lib/development.ts', 'build/lib/devtools.ts'] }],
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

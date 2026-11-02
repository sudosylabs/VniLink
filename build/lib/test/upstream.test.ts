// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { fileURLToPath } from 'node:url';
import { suite, test } from 'node:test';
import { isApplicationFile } from '../sourceScope.ts';

const root = fileURLToPath(new URL('../../../', import.meta.url));

/** The lint-staged configuration: for each glob, the commands to run on the staged files it matches. */
type StagedConfiguration = Readonly<Record<string, (files: readonly string[]) => readonly string[]>>;

/** The configuration module, which is plain JavaScript without declarations. */
function isStagedModule(value: unknown): value is { readonly default: StagedConfiguration } {
	return typeof value === 'object' && value !== null && 'default' in value && typeof value.default === 'object' && value.default !== null
		&& Object.values(value.default).every(entry => typeof entry === 'function');
}

const loaded: unknown = await import(new URL('../../../.lintstagedrc.mjs', import.meta.url).href);

assert.ok(isStagedModule(loaded));

const staged = loaded.default;

const imported = `${root}src/platform/instantiation/common/instantiationService.ts`;

const application = `${root}src/workbench/parts/titlebar/titlebarPart.ts`;

/** The commands lint-staged runs for the staged TypeScript `files`. */
const typescript = (files: readonly string[]): readonly string[] => staged['{src,test,build}/**/*.ts'](files);

suite('upstream', () => {
	test('a copied source file is not an application file', () => {
		assert.strictEqual(isApplicationFile(imported), false);
	});
	test('a source file written here is an application file', () => {
		assert.strictEqual(isApplicationFile(application), true);
	});
	for (const [index, step] of [[0, 'lint'], [1, 'format']] as const) {
		test(`staged fixes ${step} application files`, () => {
			assert.ok(typescript([imported, application])[index].includes(JSON.stringify(application)));
		});
		test(`staged fixes do not ${step} copied source`, () => {
			assert.ok(!typescript([imported, application])[index].includes(JSON.stringify(imported)));
		});
	}
	// Types span files: copied source is still type-checked, once, after the fixes.
	test('staged TypeScript is type-checked once, after the fixes', () => {
		const commands = typescript([imported, application]);
		assert.strictEqual(commands.length, 3);
		assert.strictEqual(commands[2], 'npm run typecheck');
	});
	test('staged copied source alone is only type-checked', () => {
		assert.deepStrictEqual(typescript([imported]), ['npm run typecheck']);
	});
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { suite, test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

/** The lint-staged configuration: a command list for each staged-file pattern. */
type StagedCommands = Readonly<Record<string, (files: string[]) => string | string[]>>;

suite('ci', () => {
	const root = fileURLToPath(new URL('../../../', import.meta.url));
	const read = (path: string) => readFile(join(root, path), 'utf8');
	const workflow = () => read('.github/workflows/ci.yml');
	const count = (text: string, pattern: RegExp) => (text.match(pattern) ?? []).length;
	async function stagedCommands(): Promise<StagedCommands> {
		return ((await import(pathToFileURL(join(root, '.lintstagedrc.mjs')).href)) as { readonly default: StagedCommands }).default;
	}
	for (const manager of ['debian', 'fedora', 'msys2']) {
		test(`installs the ${manager} packages from the prerequisites manifest`, async () => {
			assert.match(await workflow(), new RegExp(`prerequisites\\.ts packages ${manager} `));
		});
	}
	test('reports the doctor first in every job but Windows', async () => {
		assert.strictEqual(count(await workflow(), /npm run doctor -- --require/g), 5);
	});
	test('reports the doctor first in the Windows script', async () => {
		assert.match(await read('scripts/test-windows.ps1'), /'doctor', '--', '--require'/);
	});
	test('reads the Node version from .nvmrc', async () => {
		const ci = await workflow();
		assert.doesNotMatch(ci, /node-version:/);
		assert.match(ci, /node-version-file: \.nvmrc/);
	});
	for (const job of ['macos', 'ubuntu', 'fedora', 'windows', 'linux-container']) {
		test(`has a ${job} job`, async () => {
			assert.match(await workflow(), new RegExp(`^  ${job}:$`, 'm'));
		});
	}
	test('runs the Windows suite through the test:windows task', async () => {
		assert.match(await read('build/run.ts'), /'test:windows':.*scripts\/test-windows\.ps1/);
	});
	test('has no precommit package script', async () => {
		const manifest = JSON.parse(await read('package.json')) as { readonly scripts: Readonly<Record<string, string>> };
		assert.strictEqual(manifest.scripts.precommit, undefined);
	});
	test('runs the same sequence as validate on macOS', async () => {
		assert.match(await workflow(), /npm run validate/);
	});
	test('checks the Rust workspace through the task in both Linux jobs', async () => {
		assert.strictEqual(count(await workflow(), /npm run check:rust/g), 2);
	});
	test('the pre-commit hook runs no full suite', async () => {
		assert.doesNotMatch(await read('.husky/pre-commit'), /npm (test|run (check|validate|precommit)\b(?!:))/);
	});
	for (const [command, pattern] of [['check:hygiene', /npm run check:hygiene/], ['check:license', /npm run check:license/], ['lint-staged', /lint-staged/]] as const) {
		test(`the pre-commit hook runs ${command}`, async () => {
			assert.match(await read('.husky/pre-commit'), pattern);
		});
	}
	test('staged TypeScript ends with one project type check', async () => {
		const typescript = (await stagedCommands())['{src,test,build}/**/*.ts'];
		const commands = typescript([join(root, 'src/workbench/parts/titlebar/titlebarPart.ts')]);
		assert.ok(Array.isArray(commands));
		assert.strictEqual(commands.at(-1), 'npm run typecheck');
	});
	test('staged Rust checks the workspace formatting', async () => {
		assert.strictEqual((await stagedCommands())['crates/**/*.rs'](['crates/vnilink/src/lib.rs']), 'cargo fmt --all -- --check');
	});
	test('the pre-push hook runs validate only when vnilink.prePush is full', async () => {
		assert.match(await read('.husky/pre-push'), /vnilink\.prePush\)" = "full" \][\s\S]*npm run validate/);
	});
});

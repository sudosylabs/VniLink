// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { suite, test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

/** The lint-staged configuration: a command list for each staged-file pattern. */
type StagedCommands = Readonly<Record<string, (files: string[]) => string | string[]>>;

suite('ci', () => {
	const root = fileURLToPath(new URL('../../../', import.meta.url));
	const read = (path: string) => readFile(join(root, path), 'utf8');
	const workflow = (name: string) => read(`.github/workflows/${name}.yml`);
	const action = (name: string) => read(`.github/actions/${name}/action.yml`);
	const count = (text: string, pattern: RegExp) => (text.match(pattern) ?? []).length;
	/** Every workflow and composite action, as one text, for what may appear in any of them. */
	async function everything(): Promise<string> {
		const workflows = (await readdir(join(root, '.github/workflows'))).filter(name => name.endsWith('.yml')).map(name => `.github/workflows/${name}`);
		const actions = (await readdir(join(root, '.github/actions'))).map(name => `.github/actions/${name}/action.yml`);
		return (await Promise.all([...workflows, ...actions].map(read))).join('\n');
	}
	async function stagedCommands(): Promise<StagedCommands> {
		return ((await import(pathToFileURL(join(root, '.lintstagedrc.mjs')).href)) as { readonly default: StagedCommands }).default;
	}
	for (const [manager, file] of [['debian', '.github/actions/install-ubuntu-packages/action.yml'], ['fedora', '.github/workflows/test-linux.yml'], ['msys2', '.github/actions/setup-msys2/action.yml']] as const) {
		test(`installs the ${manager} packages from the prerequisites manifest`, async () => {
			assert.match(await read(file), new RegExp(`prerequisites\\.ts packages ${manager} `));
		});
	}
	for (const name of ['test-macos', 'test-linux-container', 'release']) {
		test(`${name} reports the doctor before it builds`, async () => {
			assert.match(await workflow(name), /npm run doctor -- --require/);
		});
	}
	test('the Linux suite reports the doctor first', async () => {
		assert.match(await action('linux-suite'), /steps:\s+- shell: bash\s+run: npm run doctor -- --require/);
	});
	test('reports the doctor first in the Windows script', async () => {
		assert.match(await read('scripts/test-windows.ps1'), /'doctor', '--', '--require'/);
	});
	test('reads the Node version from .nvmrc', async () => {
		assert.doesNotMatch(await everything(), /node-version:/);
		assert.match(await action('setup-node'), /node-version-file: \.nvmrc/);
	});
	test('every job has a time limit', async () => {
		for (const name of ['test-macos', 'test-linux', 'test-windows', 'test-linux-container', 'supply-chain', 'release', 'fuzz']) {
			const text = await workflow(name);
			assert.strictEqual(count(text, /^    timeout-minutes: \d+$/gm), count(text, /^    runs-on:/gm), name);
		}
	});
	test('ci runs on no schedule', async () => {
		assert.doesNotMatch(await workflow('ci'), /schedule:/);
	});
	test('the signing secrets are read by the release workflow alone', async () => {
		for (const name of (await readdir(join(root, '.github/workflows'))).filter(file => file !== 'release.yml')) {
			assert.doesNotMatch(await read(`.github/workflows/${name}`), /secrets\.(MACOS|APPLE|WINDOWS)_/, name);
		}
		assert.match(await workflow('release'), /^    environment: release$/m);
	});
	test('a release follows a passing ci on a push to main', async () => {
		const release = await workflow('release');
		assert.match(release, /workflow_run:\s+workflows: \[ci\]\s+types: \[completed\]\s+branches: \[main\]/);
		assert.match(release, /workflow_run\.conclusion == 'success'/);
		assert.match(release, /workflow_run\.event == 'push'/);
		assert.match(release, /ref: \$\{\{ github\.event\.workflow_run\.head_sha \|\| github\.sha \}\}/);
	});
	test('has no precommit package script', async () => {
		const manifest = JSON.parse(await read('package.json')) as { readonly scripts: Readonly<Record<string, string>> };
		assert.strictEqual(manifest.scripts.precommit, undefined);
	});
	test('checks the Rust workspace through the task in both Linux jobs', async () => {
		assert.match(await action('linux-suite'), /npm run check:rust/);
		assert.strictEqual(count(await workflow('test-linux'), /uses: \.\/\.github\/actions\/linux-suite$/gm), 2);
	});
	test('the pre-commit hook runs no full suite', async () => {
		assert.doesNotMatch(await read('.husky/pre-commit'), /npm (test|run (check|validate|precommit)\b(?!:))/);
	});
	test('staged Rust checks the workspace formatting', async () => {
		assert.strictEqual((await stagedCommands())['crates/**/*.rs'](['crates/vnilink/src/lib.rs']), 'cargo fmt --all -- --check');
	});
	test('the pre-push hook runs validate only when vnilink.prePush is full', async () => {
		assert.match(await read('.husky/pre-push'), /vnilink\.prePush\)" = "full" \][\s\S]*npm run validate/);
	});
});

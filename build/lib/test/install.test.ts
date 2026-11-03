// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { computeState, isUpToDate, saveState } from '../../npm/installState.ts';
import { nodeVersionAccepted } from '../prerequisites.ts';

const root = fileURLToPath(new URL('../../../', import.meta.url));

interface PackageJson {
	readonly engines: { readonly node: string };
	readonly volta: { readonly node: string };
	readonly scripts: Readonly<Record<string, string>>;
}

async function repositoryFile(path: string): Promise<string> {
	return readFile(join(root, path), 'utf8');
}

const nvmrc = (await repositoryFile('.nvmrc')).trim();

const packageJson = JSON.parse(await repositoryFile('package.json')) as PackageJson;

suite('install', () => {
	for (const version of ['22.19.0', '22.20.1', 'v22.19.3']) {
		test(`accepts Node ${version} where .nvmrc names 22.19.0`, () => {
			assert.ok(nodeVersionAccepted(version, '22.19.0'));
		});
	}
	for (const [version, reason] of [
		['22.18.9', 'an earlier minor of the pinned major'],
		['20.19.0', 'an earlier major'],
		['24.11.0', 'a later major, as VS Code does'],
	] as const) {
		test(`refuses Node ${version}, ${reason}, where .nvmrc names 22.19.0`, () => {
			assert.ok(!nodeVersionAccepted(version, '22.19.0'));
		});
	}
	test('refuses every Node when .nvmrc names no version', () => {
		assert.ok(!nodeVersionAccepted('22.19.0', ''));
	});
	test('npm accepts the Node versions .nvmrc accepts', () => {
		assert.strictEqual(packageJson.engines.node, `^${nvmrc}`);
	});
	test('Volta pins the version nvm reads from .nvmrc', () => {
		assert.strictEqual(packageJson.volta.node, nvmrc);
	});
	test('npm refuses a Node too old to run the TypeScript preinstall', async () => {
		assert.match(await repositoryFile('.npmrc'), /^engine-strict=true$/m);
	});
	test('the Linux image runs a Node .nvmrc accepts', async () => {
		const dockerNode = /FROM node:(\d+\.\d+\.\d+)/.exec(await repositoryFile('build/containers/Dockerfile.linux'))?.[1];
		assert.ok(dockerNode !== undefined && nodeVersionAccepted(dockerNode, nvmrc), `the Linux image's Node ${String(dockerNode)} must satisfy .nvmrc ${nvmrc}`);
	});
	test('npm runs the preinstall task before installing', () => {
		assert.strictEqual(packageJson.scripts.preinstall, 'node build/npm/preinstall.ts');
	});
	test('npm runs the postinstall task after installing', () => {
		assert.strictEqual(packageJson.scripts.postinstall, 'node build/npm/postinstall.ts');
	});
	suite('install state', () => {
		let work: string;
		const writePackage = (script: string, dependency: string): Promise<void> => writeFile(join(work, 'package.json'), JSON.stringify({ name: 'x', scripts: { a: script }, devDependencies: { b: dependency } }));
		beforeEach(async () => {
			work = await mkdtemp(join(tmpdir(), 'vnilink-install-state-'));
			await mkdir(join(work, 'node_modules'));
			await writePackage('1', '1');
			await writeFile(join(work, '.nvmrc'), '22.19.0\n');
		});
		afterEach(async () => {
			await rm(work, { recursive: true, force: true });
		});
		test('is stale before any state is saved', async () => {
			assert.strictEqual(await isUpToDate(work), false);
		});
		test('is up to date once saved', async () => {
			await saveState(work);
			assert.strictEqual(await isUpToDate(work), true);
		});
		test('stays up to date when only scripts change, which install nothing', async () => {
			await saveState(work);
			await writePackage('2', '1');
			assert.strictEqual(await isUpToDate(work), true);
		});
		test('is stale when a dependency changes', async () => {
			await saveState(work);
			await writePackage('1', '2');
			assert.strictEqual(await isUpToDate(work), false);
		});
		test('is stale when the Rust toolchain changes, so the new toolchain is prepared', async () => {
			await saveState(work);
			await writeFile(join(work, 'rust-toolchain.toml'), '[toolchain]\nchannel = "1.92.0"\n');
			assert.strictEqual(await isUpToDate(work), false);
		});
		test('records the running Node version', async () => {
			assert.strictEqual((await computeState(work)).nodeVersion, process.versions.node);
		});
	});
});

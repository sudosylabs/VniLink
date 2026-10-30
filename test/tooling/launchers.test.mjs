// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { access, constants, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { preLaunch } from '../../build/lib/preLaunch.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

test('preLaunch installs when node_modules is missing and builds only when the addon is stale', async () => {
	const runs = [];
	const run = async (command, args) => {
		runs.push([command, ...args].join(' '));
	};
	const fresh = async () => undefined;
	const stale = async () => {
		throw new Error('stale');
	};
	await preLaunch({ root: '/repo', environment: {}, exists: async () => true, assertNativeFresh: fresh, run });
	assert.deepEqual(runs, []);
	await preLaunch({ root: '/repo', environment: {}, exists: async () => false, assertNativeFresh: stale, run });
	assert.deepEqual(runs, ['npm ci', `${process.execPath} ${join('/repo', 'build/run.ts')} build`]);
	runs.length = 0;
	await preLaunch({ root: '/repo', environment: { VNILINK_SKIP_PRELAUNCH: '1' }, exists: async () => false, assertNativeFresh: stale, run });
	assert.deepEqual(runs, []);
	await assert.rejects(preLaunch({
		root: '/repo', environment: {}, exists: async () => true, assertNativeFresh: stale, run: async () => {
			throw new Error('build failed');
		}
	}), /build failed/);
});

test('the launchers prepare the checkout, then run their task with the arguments', async () => {
	for (const [script, task] of [['code', 'dev'], ['test', 'test:ui']]) {
		const shell = await readFile(join(root, `scripts/${script}.sh`), 'utf8');
		assert.match(shell, new RegExp(`node build/lib/preLaunch\\.ts\\nexec node build/run\\.ts ${task} "\\$@"\\n$`));
		await access(join(root, `scripts/${script}.sh`), constants.X_OK);
		await promisify(execFile)('bash', ['-n', join(root, `scripts/${script}.sh`)]);
		const batch = await readFile(join(root, `scripts/${script}.bat`), 'utf8');
		assert.match(batch, /node build\/lib\/preLaunch\.ts \|\| \(/);
		assert.match(batch, new RegExp(`node build/run\\.ts ${task} %\\*`));
	}
	const scripts = JSON.parse(await readFile(join(root, 'package.json'), 'utf8')).scripts;
	assert.equal(scripts.dev, 'node build/lib/preLaunch.ts && node build/run.ts dev');
	assert.equal(scripts['test:ui'], 'node build/lib/preLaunch.ts && node build/run.ts test:ui');
	assert.equal(scripts['sandbox:linux'], 'node build/run.ts sandbox:linux');
	assert.equal(scripts.doctor, 'node build/run.ts doctor');
});

test('preLaunch imports only Node modules: it installs the dependencies', async () => {
	const source = await readFile(join(root, 'build/lib/preLaunch.ts'), 'utf8');
	for (const [, specifier] of source.matchAll(/^import (?!type )[^;]*from '([^']+)'/gm)) {
		assert(specifier.startsWith('node:') || specifier === './native.ts', `preLaunch.ts imports ${specifier}`);
	}
});

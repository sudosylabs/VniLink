// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { access, constants, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { isWindows } from '../platform.ts';
import { preLaunch, type PreLaunchOptions } from '../preLaunch.ts';

const root = fileURLToPath(new URL('../../../', import.meta.url));

/** Runs preLaunch on `/repo`, recording each command it runs instead of running it. */
async function commandsRun(options: Omit<PreLaunchOptions, 'root' | 'run'>): Promise<string[]> {
	const runs: string[] = [];
	await preLaunch({
		root: '/repo', ...options, run: (command, args) => {
			runs.push([command, ...args].join(' '));
			return Promise.resolve();
		}
	});
	return runs;
}

const present = (): Promise<boolean> => Promise.resolve(true);

const absent = (): Promise<boolean> => Promise.resolve(false);

const fresh = (): Promise<void> => Promise.resolve();

const stale = (): Promise<void> => Promise.reject(new Error('stale'));

suite('launchers', () => {
	test('preLaunch runs nothing when the dependencies are installed and the addon is fresh', async () => {
		assert.deepStrictEqual(await commandsRun({ environment: {}, exists: present, assertNativeFresh: fresh }), []);
	});
	test('preLaunch installs the dependencies and builds when node_modules is missing and the addon is stale', async () => {
		assert.deepStrictEqual(await commandsRun({ environment: {}, exists: absent, assertNativeFresh: stale }), ['npm ci', `${process.execPath} ${join('/repo', 'build/run.ts')} build`]);
	});
	test('preLaunch runs nothing when VNILINK_SKIP_PRELAUNCH is set', async () => {
		assert.deepStrictEqual(await commandsRun({ environment: { VNILINK_SKIP_PRELAUNCH: '1' }, exists: absent, assertNativeFresh: stale }), []);
	});
	test('preLaunch rejects when the build fails', async () => {
		await assert.rejects(preLaunch({
			root: '/repo', environment: {}, exists: present, assertNativeFresh: stale, run: () => Promise.reject(new Error('build failed'))
		}), /build failed/);
	});
	for (const [script, task] of [['code', 'dev'], ['test', 'test:ui']] as const) {
		const shell = join(root, `scripts/${script}.sh`);
		test(`scripts/${script}.sh ends by running the ${task} task with its arguments`, async () => {
			assert.match(await readFile(shell, 'utf8'), new RegExp(`\\nexec node build/run\\.ts ${task} "\\$@"\\n$`));
		});
		test(`scripts/${script}.sh is executable`, async () => {
			await access(shell, constants.X_OK);
		});
		// The shell launchers run on macOS and Linux; Windows runs the .bat ones and may have no bash.
		test(`scripts/${script}.sh parses under bash`, { skip: isWindows && 'the .sh launchers do not run on Windows' }, async () => {
			await promisify(execFile)('bash', ['-n', shell]);
		});
		test(`scripts/${script}.bat runs the ${task} task with its arguments`, async () => {
			assert.match(await readFile(join(root, `scripts/${script}.bat`), 'utf8'), new RegExp(`node build/run\\.ts ${task} %\\*`));
		});
	}
	// npm's own lifecycle scripts prepare the install; every other script is the build task of its name.
	const manifest = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')) as { scripts: Record<string, string> };
	for (const [name, command] of Object.entries(manifest.scripts).filter(([name]) => !['preinstall', 'postinstall', 'prepare'].includes(name))) {
		test(`npm run ${name} is the build task ${name}`, () => {
			assert.strictEqual(command, `node build/run.ts ${name}`);
		});
	}
});

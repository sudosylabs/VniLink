// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { suite, test } from 'node:test';
import { selfResponsible, type Launch } from '../responsibility.ts';

const root = fileURLToPath(new URL('../../../', import.meta.url));

interface Outcome {
	readonly code: number | null;
	readonly signal: NodeJS.Signals | null;
	readonly stdout: string;
	readonly stderr: string;
}

/** Runs the launch and collects what a supervisor sees: streams and how it ended. */
function launch(command: string, args: readonly string[]): Promise<Outcome> {
	return new Promise((resolve, reject) => {
		const child = spawn(command, args, { stdio: ['ignore', 'pipe', 'pipe'] });
		let stdout = '';
		let stderr = '';
		child.stdout.setEncoding('utf8').on('data', (chunk: string) => {
			stdout += chunk;
		});
		child.stderr.setEncoding('utf8').on('data', (chunk: string) => {
			stderr += chunk;
		});
		child.once('error', reject);
		child.once('close', (code, signal) => {
			resolve({ code, signal, stdout, stderr });
		});
	});
}

function launcher(program: string, args: readonly string[]): Promise<Launch> {
	return selfResponsible(root, program, args, AbortSignal.timeout(60000));
}

const macOnly = { skip: process.platform !== 'darwin' };

suite('responsibility', () => {
	for (const platform of ['win32', 'linux'] as const) {
		test(`starts the executable as it is on ${platform}`, async () => {
			assert.deepStrictEqual(await selfResponsible(root, '/app/vnilink', ['/repo', '--flag'], AbortSignal.timeout(1000), platform), { command: '/app/vnilink', args: ['/repo', '--flag'] });
		});
	}
	test('starts the program through the disclaiming launcher on macOS', macOnly, async () => {
		const { command, args } = await launcher('/bin/sh', ['-c', 'exit 0']);
		assert.match(command, /\.build\/dev\/disclaim$/);
		assert.deepStrictEqual(args, ['/bin/sh', '-c', 'exit 0']);
	});
	test('keeps the program\'s streams and exit code on macOS', macOnly, async () => {
		const { command, args } = await launcher('/bin/sh', ['-c', 'echo out; echo err >&2; exit 42']);
		// 42 is the development relaunch request; it must reach the supervisor as it is.
		assert.deepStrictEqual(await launch(command, args), { code: 42, signal: null, stdout: 'out\n', stderr: 'err\n' });
	});
	test('exits with 127 naming a program that cannot start on macOS', macOnly, async () => {
		const { command } = await launcher('/bin/sh', []);
		const missing = await launch(command, ['/does/not/exist']);
		assert.strictEqual(missing.code, 127);
		assert.match(missing.stderr, /\/does\/not\/exist/);
	});
	test('stops the program with the launcher when the supervisor stops the process group on macOS', macOnly, async () => {
		const { command, args } = await launcher('/bin/sh', ['-c', 'echo ready; exec /bin/sleep 30']);
		const sleeper = spawn(command, args, { detached: true, stdio: ['ignore', 'pipe', 'ignore'] });
		const stopped = once(sleeper, 'exit');
		await once(sleeper.stdout, 'data');
		assert.ok(sleeper.pid !== undefined);
		process.kill(-sleeper.pid, 'SIGTERM');
		assert.deepStrictEqual(await stopped, [null, 'SIGTERM']);
	});
});

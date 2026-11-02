// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { ChildProcess } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { afterEach, beforeEach, suite, test } from 'node:test';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { assertNativeFresh, compileNative, stageNative, type NativeIdentity, type NativeOptions } from '../native.ts';
import { isMacintosh, isWindows } from '../platform.ts';
import { ProcessRunner } from '../processes.ts';
import { define, execute, parallel, series, type Task } from '../task.ts';
import { agentSides, launchAgentSide, selectPeer, selectRuns, selectServices, testkitServices } from '../test.ts';

const root = join(import.meta.dirname, '../../..');

/** Resolves once `child` has started, rejects when it cannot. */
function spawned(child: ChildProcess): Promise<void> {
	return new Promise((resolve, reject) => {
		child.once('spawn', resolve);
		child.once('error', reject);
	});
}

/** A Node process that runs until it is stopped. */
const forever = ['-e', 'setInterval(() => {}, 1000)'];

suite('build tasks', () => {
	suite('task composition', () => {
		const signal = new AbortController().signal;
		let events: string[];
		const step = (name: string): Task => define(name, () => {
			events.push(name);
			return Promise.resolve();
		});
		beforeEach(() => {
			events = [];
		});
		test('series runs its first step first and its last step last', async () => {
			await execute(define('all', series(step('first'), parallel(step('left'), step('right')), step('last'))), signal);
			assert.strictEqual(events[0], 'first');
			assert.strictEqual(events.at(-1), 'last');
		});
		test('parallel runs every task between the steps around it', async () => {
			await execute(define('all', series(step('first'), parallel(step('left'), step('right')), step('last'))), signal);
			assert.deepStrictEqual(events.slice(1, 3).sort(), ['left', 'right']);
		});
		test('a failing parallel task fails the composite with its error', async () => {
			await assert.rejects(execute(parallel(define('bad', () => Promise.reject(new Error('failed'))), step('other')), signal), /failed/);
		});
		test('a failing step stops the later steps of a series', async () => {
			await assert.rejects(execute(series(define('bad', () => Promise.reject(new Error('failed'))), step('downstream')), signal), /failed/);
			assert.deepStrictEqual(events, []);
		});
		test('parallel reports a failure only after every sibling has settled', async () => {
			let release!: () => void;
			const released = new Promise<void>(resolve => {
				release = resolve;
			});
			let sibling = false;
			// The sibling finishes a macrotask after the failure, later than any promise reaction to it.
			const failing = define('bad', () => {
				setImmediate(release);
				return Promise.reject(new Error('failed'));
			});
			const slow = define('slow', async () => {
				await released;
				sibling = true;
			});
			await assert.rejects(execute(parallel(failing, slow), signal), /failed/);
			assert.strictEqual(sibling, true);
		});
		test('an aborted signal stops a series before its first step', async () => {
			const controller = new AbortController();
			controller.abort(new Error('stopped'));
			await assert.rejects(execute(series(step('never')), controller.signal), /stopped/);
			assert.deepStrictEqual(events, []);
		});
	});
	suite('native addon', () => {
		const library = isWindows ? 'vnilink_node.dll' : isMacintosh ? 'libvnilink_node.dylib' : 'libvnilink_node.so';
		const options: NativeOptions = { profile: 'dev' };
		const foreignTarget = isWindows ? 'aarch64-apple-darwin' : 'x86_64-pc-windows-gnu';
		let checkout: string;
		let out: string;
		let built: NativeIdentity;
		const processes = {
			run: async (): Promise<number> => {
				await writeFile(join(checkout, 'target/electron/debug', library), 'binary');
				return 0;
			}
		};
		beforeEach(async () => {
			checkout = await mkdtemp(join(tmpdir(), 'vnilink-native-identity-'));
			out = join(checkout, 'out');
			for (const directory of ['crates', 'vendor', '.cargo', 'out/native', 'target/electron/debug']) {
				await mkdir(join(checkout, directory), { recursive: true });
			}
			for (const filename of ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'crates/lib.rs']) {
				await writeFile(join(checkout, filename), 'initial');
			}
			built = await compileNative(checkout, processes, options);
			await stageNative(checkout, out, options, built);
		});
		afterEach(async () => {
			await rm(checkout, { recursive: true, force: true });
		});
		test('refuses a target Electron cannot load on this host', async () => {
			await assert.rejects(compileNative('/unused', { run: () => Promise.resolve(0) }, { profile: 'dev', target: foreignTarget }), /current host/);
		});
		test('runs no build for a target Electron cannot load on this host', async () => {
			let invoked = false;
			await compileNative('/unused', {
				run: () => {
					invoked = true;
					return Promise.resolve(0);
				}
			}, { profile: 'dev', target: foreignTarget }).catch(() => undefined);
			assert.strictEqual(invoked, false);
		});
		test('the staged addon is fresh', async () => {
			await assertNativeFresh(checkout, out, options);
		});
		test('a tampered binary is stale', async () => {
			await writeFile(join(checkout, 'out/native/vnilink.node'), 'tampered');
			await assert.rejects(assertNativeFresh(checkout, out, options), /stale/);
		});
		test('staging again replaces a tampered binary', async () => {
			await writeFile(join(checkout, 'out/native/vnilink.node'), 'tampered');
			await stageNative(checkout, out, options, built);
			await assertNativeFresh(checkout, out, options);
		});
		test('a changed source makes the addon stale', async () => {
			await writeFile(join(checkout, 'crates/lib.rs'), 'changed');
			await assert.rejects(assertNativeFresh(checkout, out, options), /stale/);
		});
		test('staging refuses sources changed during the build', async () => {
			await writeFile(join(checkout, 'crates/lib.rs'), 'changed');
			await assert.rejects(stageNative(checkout, out, options, built), /changed during build/);
		});
		test('building again after a source change makes the addon fresh', async () => {
			await writeFile(join(checkout, 'crates/lib.rs'), 'changed');
			await stageNative(checkout, out, options, await compileNative(checkout, processes, options));
			await assertNativeFresh(checkout, out, options);
		});
		test('agent instructions beside the sources leave the addon fresh', async () => {
			await writeFile(join(checkout, 'crates/AGENTS.md'), 'instructions');
			await assertNativeFresh(checkout, out, options);
		});
		test('a stamp naming another target is stale', async () => {
			const stamp: unknown = JSON.parse(await readFile(join(checkout, 'out/native/build.json'), 'utf8'));
			assert.ok(typeof stamp === 'object' && stamp !== null);
			await writeFile(join(checkout, 'out/native/build.json'), JSON.stringify({ ...stamp, target: 'other' }));
			await assert.rejects(assertNativeFresh(checkout, out, options), /stale/);
		});
	});
	suite('ProcessRunner', () => {
		let runner: ProcessRunner;
		beforeEach(() => {
			runner = new ProcessRunner();
		});
		afterEach(async () => {
			await runner.stop();
		});
		test('run rejects a command that cannot be spawned', async () => {
			await assert.rejects(runner.run('vnilink-command-does-not-exist', []));
		});
		test('run rejects a non-zero exit, naming the code', async () => {
			await assert.rejects(runner.run(process.execPath, ['-e', 'process.exit(7)']), /7/);
		});
		test('stop ends a running worker', async () => {
			const { child } = runner.start(process.execPath, forever);
			await spawned(child);
			await runner.stop();
			assert.notStrictEqual(child.exitCode ?? child.signalCode, null);
		});
		test('stop can be called again once stopped', async () => {
			const { child } = runner.start(process.execPath, forever);
			await spawned(child);
			await runner.stop();
			await runner.stop();
		});
		test('a stopped runner refuses to start a process', async () => {
			await runner.stop();
			assert.throws(() => runner.start(process.execPath, []), /closed/);
		});
		test('an aborted signal stops the processes a runner owns', async () => {
			const controller = new AbortController();
			const owned = new ProcessRunner(controller.signal);
			const { child, result } = owned.start(process.execPath, forever);
			await spawned(child);
			controller.abort();
			await assert.rejects(result);
		});
		test('a runner whose signal aborted refuses to start a process', () => {
			const controller = new AbortController();
			const owned = new ProcessRunner(controller.signal);
			controller.abort();
			assert.throws(() => owned.start(process.execPath, []), /closed/);
		});
		test('stop ends a descendant of a worker', { timeout: 10_000 }, async () => {
			// The descendant writes to the worker's output, which ends only once both have exited.
			const source = `const {spawn}=require('node:child_process'); const child=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:['ignore','inherit','ignore']}); console.log(child.pid); setInterval(()=>{},1000);`;
			const { child } = runner.start(process.execPath, ['-e', source], { stdio: ['ignore', 'pipe', 'pipe'] });
			const stdout = child.stdout;
			assert.ok(stdout);
			const descendant = await new Promise<number>((resolve, reject) => {
				stdout.once('data', (value: Buffer) => {
					resolve(Number(value.toString().trim()));
				});
				child.once('error', reject);
			});
			const ended = new Promise<void>(resolve => {
				stdout.once('end', resolve);
				stdout.resume();
			});
			try {
				process.kill(descendant, 0);
				await runner.stop();
				await ended;
			} finally {
				try {
					process.kill(descendant, 'SIGKILL');
				} catch {
					// Already gone.
				}
			}
		});
	});
	suite('test kit selection', () => {
		for (const kind of ['tests', 'benches'] as const) {
			for (const service of ['files', 'desktop']) {
				test(`the test kit has ${kind} for ${service}`, async () => {
					assert.ok((await testkitServices(root, kind)).includes(service));
				});
			}
		}
		test('selectServices runs a named service alone', async () => {
			assert.deepStrictEqual(selectServices(await testkitServices(root, 'tests'), ['files']), ['files']);
		});
		test('selectServices runs every service when none is named', async () => {
			const tests = await testkitServices(root, 'tests');
			assert.deepStrictEqual(selectServices(tests, []), tests);
		});
		test('selectServices refuses a service without a suite and names those with one', () => {
			assert.throws(() => selectServices(['desktop', 'files'], ['files', 'fax']), /No suite for fax\. Services: .*files/);
		});
		test('selectRuns stresses a suite 25 times by default, without load', () => {
			assert.deepStrictEqual(selectRuns(['files']), { times: 25, load: false, services: ['files'] });
		});
		test('selectRuns takes --times and leaves the services', () => {
			assert.deepStrictEqual(selectRuns(['--times', '3', 'files']), { times: 3, load: false, services: ['files'] });
		});
		test('selectRuns takes --load wherever it stands', () => {
			assert.deepStrictEqual(selectRuns(['files', '--load', '--times', '3', 'camera']), { times: 3, load: true, services: ['files', 'camera'] });
		});
		for (const args of [['files', '--times'], ['--times', '0']]) {
			test(`selectRuns refuses ${args.join(' ')}`, () => {
				assert.throws(() => selectRuns(args), /positive whole number/);
			});
		}
		test('selectPeer leaves services alone without --peer', () => {
			assert.deepStrictEqual(selectPeer(['files', 'desktop']), { services: ['files', 'desktop'], env: {} });
		});
		test('selectPeer names the agent to the suite and keeps the services', () => {
			assert.deepStrictEqual(selectPeer(['files', '--peer', 'agent.vnl', '--peer-check', '042123'], '/work'), {
				services: ['files'],
				env: { VNILINK_PEER: resolve('/work', 'agent.vnl'), VNILINK_PEER_CHECK: '042123' },
			});
		});
		test('selectPeer keeps an absolute agent file where it is', () => {
			assert.strictEqual(selectPeer(['--peer-check', '042123', '--peer', '/elsewhere/agent.vnl', 'files'], '/work').env.VNILINK_PEER, resolve('/elsewhere/agent.vnl'));
		});
		const refused: readonly (readonly [readonly string[], RegExp])[] = [
			[['files', '--peer', 'agent.vnl'], /--peer and --peer-check go together/],
			[['--peer-check', '042123'], /--peer and --peer-check go together/],
			[['files', '--peer'], /--peer needs a value/],
			[['--peer', '--peer-check', '042123'], /--peer needs a value/],
			[['--peer', 'agent.vnl', '--peer-check', '42'], /six digits/],
			[['--peer', 'agent.vnl', '--peer-check', 'abc'], /six digits/],
		];
		for (const [args, error] of refused) {
			test(`selectPeer refuses ${args.join(' ')}`, () => {
				assert.throws(() => selectPeer(args), error);
			});
		}
	});
	suite('agent sides', () => {
		for (const side of agentSides) {
			test(`the test kit's agent has a ${side} scenario`, async () => {
				const scenarios = await readFile(join(root, 'crates/vnilink-testkit/src/peer/scenarios.rs'), 'utf8');
				assert.match(scenarios, new RegExp(`"${side}" =>`));
			});
		}
		for (const args of [[], ['latency-sender'], ['latency-host', 'latency-viewer'], ['../latency-host']]) {
			test(`testkit:launch refuses ${JSON.stringify(args)}`, async () => {
				await assert.rejects(launchAgentSide('/root', new AbortController().signal, args), /testkit:launch takes one of latency-host, latency-viewer/);
			});
		}
	});
});

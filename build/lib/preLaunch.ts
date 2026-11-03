// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Prepares a checkout in any state for a launch, after VS Code's
 * `build/lib/preLaunch.ts`: installs the dependencies when `node_modules` is
 * missing, and builds when the staged native addon or a JavaScript bundle is
 * missing or older than its sources. The task it precedes then checks what it
 * needs and prepares the branded Electron itself. `scripts/code.*`, `scripts/test.*`,
 * `npm run dev` and `npm run test:ui` run it first; `VNILINK_SKIP_PRELAUNCH=1`
 * skips it.
 *
 * Imports only modules that use nothing but Node's own (`native.ts`,
 * `platform.ts`, `processes.ts`): it runs before the dependencies are
 * installed, so the bundle check, which needs esbuild, is imported once they
 * are.
 */
import { access } from 'node:fs/promises';
import { join } from 'node:path';
import { assertNativeFresh } from './native.ts';
import { isWindows } from './platform.ts';
import { ProcessRunner } from './processes.ts';

/** Runs `command` in `root`, stopped with `signal`. */
async function runProcess(root: string, signal: AbortSignal | undefined, command: string, args: readonly string[]): Promise<void> {
	const processes = new ProcessRunner(signal);
	try {
		// npm is a batch file on Windows, which only a shell starts.
		await processes.run(command, args, { cwd: root, shell: isWindows && command === 'npm' });
	} finally {
		await processes.stop();
	}
}

export interface PreLaunchOptions {
	readonly root: string;
	/** Stops what preLaunch started; the task's signal. */
	readonly signal?: AbortSignal;
	readonly environment?: NodeJS.ProcessEnv;
	readonly exists?: (path: string) => Promise<boolean>;
	/** Rejects when the staged addon is missing or stale; `native.ts`'s by default. */
	readonly assertNativeFresh?: (root: string) => Promise<void>;
	/** Rejects when a bundle in `out/` is missing or stale; `bundles.ts`'s by default. */
	readonly assertBundlesFresh?: (root: string) => Promise<void>;
	readonly run?: (command: string, args: readonly string[]) => Promise<void>;
}

export async function preLaunch(options: PreLaunchOptions): Promise<void> {
	const { root: checkout, environment = process.env } = options;
	const run = options.run ?? ((command: string, args: readonly string[]) => runProcess(checkout, options.signal, command, args));
	if (environment.VNILINK_SKIP_PRELAUNCH) {
		return;
	}
	const exists = options.exists ?? (path => access(path).then(() => true, () => false));
	if (!await exists(join(checkout, 'node_modules'))) {
		console.log('[preLaunch] Installing dependencies (npm ci)');
		await run('npm', ['ci']);
	}
	const assertBundlesFresh = options.assertBundlesFresh ?? (async (path: string) => (await import('./bundles.ts')).assertBundlesFresh(path));
	try {
		await (options.assertNativeFresh ?? assertNativeFresh)(checkout);
		await assertBundlesFresh(checkout);
		return;
	} catch {
		console.log('[preLaunch] The native addon or a bundle is missing or older than its sources; building (node build/run.ts build)');
	}
	await run(process.execPath, [join(checkout, 'build/run.ts'), 'build']);
}

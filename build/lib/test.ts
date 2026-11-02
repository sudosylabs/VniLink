// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Test runners that own processes: the Electron UI suite, Rust and loom tests, and the Linux validation container. */
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { scenarioNames, ScenarioSelection } from '../../test/automation/src/scenarios.ts';
import { runContainer } from './containers.ts';
import { electronExecutable } from './electron.ts';
import { assertNativeFresh, defaultNativeOptions, nativeCargo, nativeEnvironment } from './native.ts';
import { isLinux } from './platform.ts';
import { ProcessRunner } from './processes.ts';

/** Runs the selected browser and integration scenarios in Electron with a temporary profile. */
export async function runUiTests(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const selection = new ScenarioSelection(args);
	if (selection.list) {
		console.log(scenarioNames.join('\n'));
		return;
	}
	const processes = new ProcessRunner(signal);
	const profile = await mkdtemp(join(tmpdir(), 'vnilink-test-'));
	try {
		await assertNativeFresh(root);
		// Docker's default seccomp policy blocks Chromium namespace creation.
		// This exception applies only to the maintained isolated Linux test image.
		const containerFlags = isLinux && process.env.VNILINK_TEST_CONTAINER === '1' ? ['--no-sandbox'] : [];
		const electron = await electronExecutable(root, signal, { sandboxed: containerFlags.length === 0 });
		await processes.run(electron, [join(root, 'out/smoke.cjs'), ...containerFlags, ...args], { env: { ...process.env, VNILINK_TEST_PROFILE: profile } });
	} finally {
		await processes.stop();
		await rm(profile, { recursive: true, force: true });
	}
}

/** One Cargo invocation shares native dependency builds across the affected test crates. */
export async function testRust(root: string, signal: AbortSignal): Promise<void> {
	const backend = ({ darwin: 'vnilink-macos', win32: 'vnilink-windows', linux: 'vnilink-linux' } as Partial<Record<NodeJS.Platform, string>>)[process.platform];
	if (!backend) {
		throw new Error('Unsupported native test platform');
	}
	const packages = ['vnilink', 'vnilink-app', 'vnilink-media', 'vnilink-audio', 'vnilink-hevc', 'vnilink-rename', 'vnilink-log', 'vnilink-testkit', backend, 'vnilink-node'];
	const processes = new ProcessRunner(signal);
	try {
		// The addon's target and toolchain: on Windows, the GNU target with MSYS2's MinGW tools.
		const test = nativeCargo('test', ['--no-fail-fast', ...packages.flatMap(name => ['-p', name])]);
		await processes.run('cargo', test.args, { cwd: root, env: test.env });
	} finally {
		await processes.stop();
	}
}

/**
 * Where the loom build lands. Its `--cfg vnilink_loom` applies to every crate
 * it compiles, so sharing the addon's target directory would rebuild the
 * addon's release dependencies on each switch.
 */
const loomTargetDirectory = 'target/loom';

/**
 * The `loom_*` models of `vnilink`'s concurrent pieces, with the addon's
 * target and environment. Optimised, as loom runs each model once per
 * interleaving.
 */
export async function testLoom(root: string, signal: AbortSignal): Promise<void> {
	const options = defaultNativeOptions();
	const target = options.target ? ['--target', options.target] : [];
	const env = nativeEnvironment(options);
	env.RUSTFLAGS = [env.RUSTFLAGS, '--cfg vnilink_loom'].filter(Boolean).join(' ');
	const processes = new ProcessRunner(signal);
	try {
		await processes.run('cargo', ['test', '--locked', '--target-dir', loomTargetDirectory, ...target, '--release', '-p', 'vnilink', 'loom_'], { cwd: root, env });
	} finally {
		await processes.stop();
	}
}

/** Reproducible Linux validation with a read-only checkout and isolated writable volumes. */
export async function validateLinux(root: string, signal: AbortSignal): Promise<void> {
	await runContainer(root, signal, { purpose: 'linux', evidence: 'linux-validation', options: ['--shm-size=512m'] });
}

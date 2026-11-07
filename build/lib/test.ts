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
		if (isLinux) {
			await testLinuxSession(root, processes);
		}
	} finally {
		await processes.stop();
	}
}

/**
 * Ignored `vnilink-linux` tests that need a D-Bus session or an X server of
 * their own: the power inhibitors and the screen-sharing portal against fake
 * services on the bus, and the XFixes cursor on Xvfb. Ignored in the plain run
 * because a desktop's own session would answer them.
 */
const linuxSessionTests = [
	'power::bus_tests::private_bus_missing_gnome_uses_standard_screensaver_path',
	'power::bus_tests::a_display_inhibition_is_released_from_the_runtime_itself',
	'power::bus_tests::private_bus_cookie_and_fd_are_released_independently',
	'wayland::tests::real_portal_notify_transport_preserves_all_events',
	'tests::x11_tests::real_xfixes_cursor_is_independent_from_capture_and_uses_shared_shape_storage',
];

/**
 * Runs `linuxSessionTests` on a private session bus and Xvfb, as
 * `scripts/test.sh` runs the UI scenarios. One at a time: two of them own the
 * same bus name, and one expects no GNOME service where another serves it.
 */
async function testLinuxSession(root: string, processes: ProcessRunner): Promise<void> {
	const test = nativeCargo('test', ['-p', 'vnilink-linux', '--lib', '--', '--ignored', '--exact', '--test-threads=1', ...linuxSessionTests]);
	const env = { ...test.env, VNILINK_TEST_X11: 'private', VNILINK_TEST_PORTAL_BUS: 'private' };
	await processes.run('dbus-run-session', ['--', 'xvfb-run', '-a', '-s', '-screen 0 1280x1024x24', 'cargo', ...test.args], { cwd: root, env });
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

/**
 * Reproducible Linux validation with a read-only checkout and isolated writable
 * volumes. Without a GPU every canvas is shared memory until collected; the unit
 * suite's peak, about 740 MB, outgrows a smaller `/dev/shm` and the renderer stops.
 */
export async function validateLinux(root: string, signal: AbortSignal): Promise<void> {
	await runContainer(root, signal, { purpose: 'linux', evidence: 'linux-validation', options: ['--shm-size=2g'] });
}

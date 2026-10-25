/** Test runners that own processes: the Electron UI suite, Rust tests and the Linux validation container. */
import { mkdir, mkdtemp, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ScenarioSelection, scenarioNames } from '../../test/automation/scenarios.ts';
import { linuxValidationVolumes } from './containers.ts';
import { electronExecutable } from './electron.ts';
import { assertNativeFresh, defaultNativeOptions, nativeEnvironment } from './native.ts';
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
		const containerFlags = process.platform === 'linux' && process.env.VNILINK_TEST_CONTAINER === '1' ? ['--no-sandbox'] : [];
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
		const native = defaultNativeOptions();
		const target = native.target ? ['--target', native.target] : [];
		await processes.run('cargo', ['test', '--no-fail-fast', '--locked', '--target-dir', 'target/electron', ...target, ...packages.flatMap(name => ['-p', name])], { cwd: root, env: nativeEnvironment(native) });
	} finally {
		await processes.stop();
	}
}

/** The services `crates/vnilink-testkit` has a suite of `kind` for: its `tests/<service>.rs` or `benches/<service>.rs`. */
export async function testkitServices(root: string, kind: 'tests' | 'benches'): Promise<string[]> {
	const files = await readdir(join(root, 'crates/vnilink-testkit', kind));
	return files.filter(file => file.endsWith('.rs')).map(file => file.slice(0, -'.rs'.length)).sort();
}

/** The services named in `args`, every one when none is; a name without a suite is refused. */
export function selectServices(available: readonly string[], args: readonly string[]): string[] {
	const unknown = args.filter(name => !available.includes(name));
	if (unknown.length > 0) {
		throw new Error(`No suite for ${unknown.join(', ')}. Services: ${available.join(', ')}`);
	}
	return args.length > 0 ? [...args] : [...available];
}

/** Runs the functional suites of the named services, each alone with its engines on this computer. */
export async function testService(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const services = selectServices(await testkitServices(root, 'tests'), args);
	await runTestkit(root, signal, ['test', ...services.flatMap(name => ['--test', name])]);
}

/**
 * Runs the performance suites of the named services. Each run is saved under
 * `.build/bench` and compared with the previous one; a regression fails the task.
 */
export async function benchService(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const services = selectServices(await testkitServices(root, 'benches'), args);
	await runTestkit(root, signal, ['bench', ...services.flatMap(name => ['--bench', name])], { VNILINK_BENCH_DIR: join(root, '.build/bench') });
}

/** Cargo on the test kit with the addon's target and toolchain, as `testRust` runs it. */
async function runTestkit(root: string, signal: AbortSignal, command: readonly string[], env: Readonly<Record<string, string>> = {}): Promise<void> {
	const processes = new ProcessRunner(signal);
	try {
		const native = defaultNativeOptions();
		const target = native.target ? ['--target', native.target] : [];
		const [verb, ...suites] = command;
		await processes.run('cargo', [verb, '--locked', '--target-dir', 'target/electron', ...target, '-p', 'vnilink-testkit', ...suites], { cwd: root, env: { ...nativeEnvironment(native), ...env } });
	} finally {
		await processes.stop();
	}
}

/** Reproducible Linux validation with a read-only checkout and isolated writable volumes. */
export async function validateLinux(root: string, signal: AbortSignal): Promise<void> {
	const output = join(root, '.build/linux-validation', String(Date.now()));
	const container = `vnilink-validation-${String(process.pid)}-${String(Date.now())}`;
	const processes = new ProcessRunner(signal);
	let launched = false;
	await mkdir(output, { recursive: true });
	try {
		await processes.run('docker', ['info', '--format', '{{.OSType}} {{.Architecture}}']);
		await processes.run('docker', ['build', '-f', 'build/containers/Dockerfile.linux', '-t', 'vnilink-linux-validation:local', '.'], { cwd: root });
		launched = true;
		await processes.run('docker', [
			'run', '--name', container, '--shm-size=512m', '--init',
			'--mount', `type=bind,source=${root},target=/source,readonly`,
			'--mount', `type=bind,source=${output},target=/evidence`,
			...linuxValidationVolumes(root),
			'vnilink-linux-validation:local',
		]);
	} finally {
		await processes.stop();
		if (launched) {
			// Removal uses a separate owner because cancellation closes the work runner.
			const cleanup = new ProcessRunner();
			try {
				await cleanup.run('docker', ['rm', '--force', container]);
			} finally {
				await cleanup.stop();
			}
		}
		console.log(`Linux evidence: ${output}`);
	}
}

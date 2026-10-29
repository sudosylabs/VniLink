/** Test runners that own processes: the Electron UI suite, Rust tests and the Linux validation container. */
import { mkdir, mkdtemp, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { ScenarioSelection, scenarioNames } from '../../test/automation/scenarios.ts';
import { linuxValidationVolumes } from './containers.ts';
import { electronExecutable } from './electron.ts';
import { launchLatencySide } from './latency.ts';
import { assertNativeFresh, defaultNativeOptions, nativeEnvironment } from './native.ts';
import { launchPerceivedStimulus } from './perceived.ts';
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

/** A suite's run against the test agent on another computer, out of its arguments: `--peer <file.vnl>` and `--peer-check <digits>`. */
export interface PeerSelection {
	/** The arguments that name services. */
	readonly services: string[];
	/** What tells the suite about the agent: `VNILINK_PEER` and `VNILINK_PEER_CHECK`, empty without `--peer`. */
	readonly env: Readonly<Record<string, string>>;
}

/** Takes `--peer` and `--peer-check` out of `args`; each needs a value, and one needs the other. */
export function selectPeer(args: readonly string[], cwd = process.cwd()): PeerSelection {
	const services: string[] = [];
	let file: string | undefined;
	let check: string | undefined;
	for (let index = 0; index < args.length; index++) {
		const arg = args[index];
		if (arg !== '--peer' && arg !== '--peer-check') {
			services.push(arg);
			continue;
		}
		const value = args.at(++index);
		if (value === undefined || value.startsWith('--')) {
			throw new Error(`${arg} needs a value: --peer <the agent's .vnl file> --peer-check <the digits the agent printed>`);
		}
		if (arg === '--peer') {
			file = resolve(cwd, value);
		} else {
			check = value;
		}
	}
	if (file === undefined && check === undefined) {
		return { services, env: {} };
	}
	if (file === undefined || check === undefined) {
		throw new Error('--peer and --peer-check go together: the agent\'s .vnl file and the digits it printed beside it');
	}
	if (!/^\d{3}$/.test(check)) {
		throw new Error('--peer-check takes the three digits the agent printed');
	}
	return { services, env: { VNILINK_PEER: file, VNILINK_PEER_CHECK: check } };
}

/** Runs the functional suites of the named services, each alone with its engines on this computer, or against the agent named by `--peer`. */
export async function testService(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const peer = selectPeer(args);
	const services = selectServices(await testkitServices(root, 'tests'), peer.services);
	await runTestkit(root, signal, ['test', ...services.flatMap(name => ['--test', name])], { env: peer.env });
}

/**
 * Runs the performance suites of the named services. Each run is saved under
 * `.build/bench` and compared with the previous one; a regression fails the task.
 * With `--peer` a run measures against the agent on another computer, is saved
 * apart and only reports regressions.
 */
export async function benchService(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const peer = selectPeer(args);
	const services = selectServices(await testkitServices(root, 'benches'), peer.services);
	await runTestkit(root, signal, ['bench', ...services.flatMap(name => ['--bench', name])], { env: { VNILINK_BENCH_DIR: join(root, '.build/bench'), ...peer.env } });
}

/**
 * Runs the test agent (`vnilink-testkit agent`), the host half of suites and
 * measurements run with `--peer` on another computer: it writes a pairing
 * file, by default `.build/testkit/agent.vnl`, prints its digits and asks on
 * this terminal for the digits that computer's suite prints.
 */
export async function runTestkitAgent(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const out = args.includes('--out') ? [] : ['--out', join(root, '.build/testkit/agent.vnl')];
	// Measurements need the optimized build that bench:service runs. A
	// measurement's side starts through this Node (`testkit:launch`).
	await runTestkit(root, signal, ['run', '--release', '--bin', 'vnilink-testkit', '--', 'agent', ...out, ...args], { typing: true, env: { VNILINK_AGENT_NODE: process.execPath } });
}

/** The measurement sides the test agent starts from this checkout (`testkit:launch <side>`). */
export const agentSides = ['latency-host', 'latency-viewer', 'perceived-stimulus'] as const;

/**
 * Starts one side of a measurement for the test agent, which runs this task
 * from its own checkout with the side's folders in its environment, and
 * stops it when the agent closes this task's input.
 */
export async function launchAgentSide(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const [side, ...rest] = args;
	if (!agentSides.includes(side as (typeof agentSides)[number]) || rest.length > 0) {
		throw new Error(`testkit:launch takes one of ${agentSides.join(', ')}`);
	}
	const stopped = new Promise<void>(resolve => {
		process.stdin.once('end', resolve);
		process.stdin.once('close', resolve);
		process.stdin.resume();
	});
	try {
		if (side === 'perceived-stimulus') {
			await launchPerceivedStimulus(root, signal, stopped);
		} else {
			await launchLatencySide(root, signal, side === 'latency-host' ? 'host' : 'viewer', stopped);
		}
	} finally {
		process.stdin.destroy();
	}
}

interface TestkitOptions {
	readonly env?: Readonly<Record<string, string>>;
	/** Passes this terminal's typing on: the process runs in a process group of its own, which the terminal would stop when it reads. */
	readonly typing?: boolean;
}

/** Cargo on the test kit with the addon's target and toolchain, as `testRust` runs it. */
async function runTestkit(root: string, signal: AbortSignal, command: readonly string[], options: TestkitOptions = {}): Promise<void> {
	const processes = new ProcessRunner(signal);
	try {
		const native = defaultNativeOptions();
		const target = native.target ? ['--target', native.target] : [];
		const [verb, ...suites] = command;
		const launched = processes.start('cargo', [verb, '--locked', '--target-dir', 'target/electron', ...target, '-p', 'vnilink-testkit', ...suites], {
			cwd: root,
			env: { ...nativeEnvironment(native), ...options.env },
			...options.typing ? { stdio: ['pipe', 'inherit', 'inherit'] } : {},
		});
		if (options.typing && launched.child.stdin) {
			process.stdin.pipe(launched.child.stdin);
		}
		await launched.result;
	} finally {
		if (options.typing) {
			process.stdin.unpipe();
			process.stdin.pause();
		}
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

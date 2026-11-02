// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The one entry point for build, packaging, verification and test tasks:
 * `node build/run.ts <task> [arguments]`; `--help` lists them with what they
 * do. Each `package.json` script is the task of the same name. A task loads
 * its modules only when it runs, so `doctor` and `--help` work before the
 * dependencies are installed. A failure prints its cause (`VNILINK_BUILD_STACK=1`
 * adds the stack) and exits 1. Interrupting the process aborts the task, which
 * stops the processes it started and leaves published outputs intact; a
 * second interrupt exits at once.
 */
import { fileURLToPath } from 'node:url';
import { assertTaskReady, loadManifest } from './lib/prerequisites.ts';

type TaskFunction = (root: string, signal: AbortSignal, args: readonly string[]) => Promise<unknown>;

/**
 * Every task. A composite runs its steps with `step`, which checks each
 * step's prerequisites as if it were run alone; arguments go to the last step.
 */
const tasks: Readonly<Record<string, TaskFunction>> = {
	'typecheck': async (root, signal) => (await import('./lib/tools.ts')).typecheck(root, signal),
	'build': async (root, signal) => (await import('./lib/bundles.ts')).buildApplication(root, signal),
	'build:release': async (root, signal) => {
		await step('check:product');
		await step('typecheck');
		return (await import('./lib/release.ts')).buildRelease(root, signal);
	},
	'package': async (root, signal) => (await import('./lib/package.ts')).packageApplication(root, signal),
	'package:dir': async () => {
		await step('build:release');
		await step('package');
	},
	'package:installers:staged': async (root, signal) => (await import('./lib/installers.ts')).buildInstallers(root, signal),
	'package:installers': async () => {
		await step('package:dir');
		await step('package:installers:staged');
	},
	'verify:release': async root => (await import('./lib/verify.ts')).verifyRelease(root),
	'verify:package': async (root, signal) => (await import('./lib/verify.ts')).verifyPackage(root, signal),
	'verify:installers': async (root, signal) => (await import('./lib/verify.ts')).verifyInstallers(root, signal),
	'start': async (root, signal, args) => {
		await step('build');
		return (await import('./lib/development.ts')).startApplication(root, signal, args);
	},
	'dev': async (root, signal, args) => {
		await (await import('./lib/preLaunch.ts')).preLaunch({ root });
		const { DevelopmentSession } = await import('./lib/development.ts');
		return new DevelopmentSession(root, args, signal).run();
	},
	'watch': async (root, signal, args) => {
		const { DevelopmentSession } = await import('./lib/development.ts');
		return new DevelopmentSession(root, ['--watch-only', ...args], signal).run();
	},
	'test': async (_root, _signal, args) => {
		await step('build');
		await step('test:native');
		await step('test:ui', args);
	},
	'test:ui': async (root, signal, args) => {
		await (await import('./lib/preLaunch.ts')).preLaunch({ root });
		// The whole UI suite starts with the renderer unit tests; a scenario selection runs only those scenarios.
		if (!args.length) {
			await step('test:unit:electron');
		}
		return (await import('./lib/test.ts')).runUiTests(root, signal, args);
	},
	'test:native': async (root, signal) => (await import('./lib/tools.ts')).runNodeTests(root, signal, 'test/native/*.test.mjs'),
	'test:tooling': async (root, signal) => {
		const { runNodeTests } = await import('./lib/tools.ts');
		await runNodeTests(root, signal, 'build/lib/test/*.test.ts');
		await runNodeTests(root, signal, 'test/tooling/*.test.mjs');
	},
	'test:unit': async (root, signal, args) => (await import('./lib/unitTests.ts')).runNodeUnitTests(root, signal, args),
	'test:unit:electron': async (root, signal, args) => (await import('./lib/unitTests.ts')).runElectronUnitTests(root, signal, args),
	'test:rust': async (root, signal) => (await import('./lib/test.ts')).testRust(root, signal),
	'test:service': async (root, signal, args) => (await import('./lib/test.ts')).testService(root, signal, args),
	'test:stress': async (root, signal, args) => (await import('./lib/test.ts')).stressService(root, signal, args),
	'bench:service': async (root, signal, args) => (await import('./lib/test.ts')).benchService(root, signal, args),
	'testkit:agent': async (root, signal, args) => (await import('./lib/test.ts')).runTestkitAgent(root, signal, args),
	'testkit:launch': async (root, signal, args) => (await import('./lib/test.ts')).launchAgentSide(root, signal, args),
	'test:linux': async (root, signal) => (await import('./lib/test.ts')).validateLinux(root, signal),
	'test:windows': async (root, signal, args) => (await import('./lib/tools.ts')).runTool(root, signal, 'powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'scripts/test-windows.ps1', ...args]),
	'sandbox:linux': async (root, signal) => (await import('./lib/electron.ts')).allowLinuxSandbox(root, signal),
	'measure:impaired': async (root, signal, args) => (await import('./lib/impaired.ts')).measureImpairedInContainer(root, signal, args),
	'measure:impaired:linux': async (root, signal, args) => (await import('./lib/impaired.ts')).measureImpaired(root, signal, args),
	'measure:latency': async (root, signal, args) => (await import('./lib/latency.ts')).measureLatency(root, signal, args),
	'check': async () => {
		for (const name of ['check:product', 'check:hygiene', 'check:license', 'check:upstream', 'check:unused', 'format:check', 'lint', 'typecheck', 'test:tooling', 'test:unit']) {
			await step(name);
		}
	},
	'check:product': async root => (await import('./lib/product.ts')).checkProduct(root),
	'check:hygiene': async root => (await import('./lib/hygiene.ts')).checkHygiene(root),
	'check:license': async root => (await import('./lib/license.ts')).checkLicense(root),
	'check:upstream': async (root, _signal, args) => (await import('./lib/vscode/dependencies.ts')).verifyUpstream(root, args),
	'check:unused': async (root, _signal, args) => (await import('./lib/unused.ts')).checkUnused(root, args),
	'check:rust': async (root, signal) => (await import('./lib/tools.ts')).checkRust(root, signal),
	'validate': async () => {
		await step('check');
		await step('check:rust');
		await step('test:rust');
		await step('test');
	},
	'lint': async () => {
		await step('lint:ts');
		await step('lint:css');
	},
	'lint:ts': async (root, signal) => (await import('./lib/tools.ts')).lintTypeScript(root, signal),
	'lint:css': async (root, signal) => (await import('./lib/tools.ts')).lintStyles(root, signal),
	'lint:fix': async (root, signal) => {
		const { lintStyles, lintTypeScript } = await import('./lib/tools.ts');
		await lintTypeScript(root, signal, true);
		await lintStyles(root, signal, true);
	},
	'format': async (_root, _signal, args) => (await import('./lib/format.ts')).formatFiles(['--write', ...args]),
	'format:check': async (_root, _signal, args) => (await import('./lib/format.ts')).formatFiles(['--check', ...args]),
	'license:apply': async (root, _signal, args) => (await import('./lib/license.ts')).applyLicenseCommand(root, args),
	'assets:tray': async (root, signal) => (await import('./lib/trayAssets.ts')).renderTrayAssets(root, signal),
	'vscode': async (root, _signal, args) => (await import('./lib/vscode/import.ts')).vscodeCommand(root, args),
	'doctor': async (root, _signal, args) => (await import('./lib/doctor.ts')).doctor(root, args),
	'docs:prerequisites': async (root, _signal, args) => (await import('./lib/prerequisitesDocs.ts')).updatePrerequisiteDocs(root, args),
};

const root = fileURLToPath(new URL('../', import.meta.url));

const controller = new AbortController();

/** Runs task `name` after checking what it needs here (`build/prerequisites.json`). */
async function step(name: string, args: readonly string[] = []): Promise<unknown> {
	const task = Object.hasOwn(tasks, name) ? tasks[name] : undefined;
	if (!task) {
		throw new Error(`Unknown task ${name}`);
	}
	controller.signal.throwIfAborted();
	await assertTaskReady(root, name);
	return task(root, controller.signal, args);
}

async function usage(): Promise<string> {
	const manifest = await loadManifest();
	const width = Math.max(...Object.keys(tasks).map(name => name.length));
	const lines = Object.keys(tasks).sort().map(name => `  ${name.padEnd(width)}  ${Object.hasOwn(manifest.tasks, name) ? manifest.tasks[name].description : ''}`);
	return `Usage: node build/run.ts <task> [arguments]\n\nTasks:\n${lines.join('\n')}`;
}

const [name = '', ...args] = process.argv.slice(2);

if (name === '--help' || name === '-h' || name === 'help') {
	console.log(await usage());
	process.exit(0);
}

if (!Object.hasOwn(tasks, name)) {
	console.error(`${name ? `Unknown task ${name}.\n\n` : ''}${await usage()}`);
	process.exit(2);
}

for (const signal of ['SIGINT', 'SIGTERM'] as const) {
	process.on(signal, () => {
		if (controller.signal.aborted) {
			console.error(`\nInterrupted again by ${signal}; exiting without waiting for cleanup.`);
			process.exit(130);
		}
		console.error(`\nInterrupted by ${signal}; stopping. Interrupt again to exit at once.`);
		controller.abort(new Error(`Interrupted by ${signal}`));
	});
}

try {
	await step(name, args);
} catch (error) {
	// A failure names its cause; VNILINK_BUILD_STACK=1 adds where it was thrown.
	console.error(process.env.VNILINK_BUILD_STACK === '1' || !(error instanceof Error) ? error : `${name} failed: ${error.message}`);
	process.exitCode = 1;
}

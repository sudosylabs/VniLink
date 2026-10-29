/**
 * The one entry point for build, packaging, verification and test tasks:
 * `node build/run.ts <task> [arguments]`. Interrupting the process aborts the
 * task, which stops the processes it started and leaves published outputs intact.
 */
import { fileURLToPath } from 'node:url';
import { buildApplication } from './lib/bundles.ts';
import { DevelopmentSession, startApplication } from './lib/development.ts';
import { doctor } from './lib/doctor.ts';
import { allowLinuxSandbox } from './lib/electron.ts';
import { checkHygiene } from './lib/hygiene.ts';
import { measureImpaired, measureImpairedInContainer } from './lib/impaired.ts';
import { measureLatency } from './lib/latency.ts';
import { comparePerceived, measurePerceived } from './lib/perceived.ts';
import { buildInstallers } from './lib/installers.ts';
import { renderTrayAssets } from './lib/trayAssets.ts';
import { packageApplication } from './lib/package.ts';
import { assertTaskReady } from './lib/prerequisites.ts';
import { updatePrerequisiteDocs } from './lib/prerequisitesDocs.ts';
import { checkProduct } from './lib/product.ts';
import { buildRelease } from './lib/release.ts';
import { benchService, launchAgentSide, runTestkitAgent, runUiTests, stressService, testRust, testService, validateLinux } from './lib/test.ts';
import { verifyInstallers, verifyPackage, verifyRelease } from './lib/verify.ts';
import { formatFiles } from './format.ts';
import { verifyUpstream } from './vscodeDependencies.ts';
import { vscodeCommand } from './vscodeImport.ts';

type TaskFunction = (root: string, signal: AbortSignal, args: readonly string[]) => Promise<unknown>;

const tasks: Readonly<Record<string, TaskFunction>> = {
	'build': (root, signal) => buildApplication(root, signal),
	'build:release': (root, signal) => buildRelease(root, signal),
	'package': (root, signal) => packageApplication(root, signal),
	'package:installers': (root, signal) => buildInstallers(root, signal),
	'verify:release': root => verifyRelease(root),
	'verify:package': (root, signal) => verifyPackage(root, signal),
	'verify:installers': (root, signal) => verifyInstallers(root, signal),
	'start': (root, signal, args) => startApplication(root, signal, args),
	'dev': (root, signal, args) => new DevelopmentSession(root, args, signal).run(),
	'watch': (root, signal, args) => new DevelopmentSession(root, ['--watch-only', ...args], signal).run(),
	'test:ui': (root, signal, args) => runUiTests(root, signal, args),
	'test:rust': (root, signal) => testRust(root, signal),
	'test:service': (root, signal, args) => testService(root, signal, args),
	'test:stress': (root, signal, args) => stressService(root, signal, args),
	'bench:service': (root, signal, args) => benchService(root, signal, args),
	'testkit:agent': (root, signal, args) => runTestkitAgent(root, signal, args),
	'testkit:launch': (root, signal, args) => launchAgentSide(root, signal, args),
	'test:linux': (root, signal) => validateLinux(root, signal),
	'sandbox:linux': (root, signal) => allowLinuxSandbox(root, signal),
	'measure:impaired': (root, signal, args) => measureImpairedInContainer(root, signal, args),
	'measure:impaired:linux': (root, signal, args) => measureImpaired(root, signal, args),
	'measure:latency': (root, signal, args) => measureLatency(root, signal, args),
	'measure:perceived': (root, signal, args) => args[0] === '--compare' ? comparePerceived(args.slice(1)) : measurePerceived(root, signal, args),
	'check:product': root => checkProduct(root),
	'check:hygiene': root => checkHygiene(root),
	'assets:tray': (root, signal) => renderTrayAssets(root, signal),
	'check:upstream': () => verifyUpstream(),
	'format': (_root, _signal, args) => formatFiles(args),
	'vscode': (_root, _signal, args) => vscodeCommand(args),
	'doctor': (root, _signal, args) => doctor(root, args),
	'docs:prerequisites': (root, _signal, args) => updatePrerequisiteDocs(root, args),
};

const [name = '', ...args] = process.argv.slice(2);

const task = Object.hasOwn(tasks, name) ? tasks[name] : undefined;

if (!task) {
	console.error(`Usage: node build/run.ts <task> [arguments]\nTasks: ${Object.keys(tasks).join(', ')}`);
	process.exit(2);
}

const controller = new AbortController();

for (const signal of ['SIGINT', 'SIGTERM'] as const) {
	process.once(signal, () => {
		controller.abort(new Error(`Interrupted by ${signal}`));
	});
}

const root = fileURLToPath(new URL('../', import.meta.url));

try {
	// Every task first checks what it needs here (build/prerequisites.json).
	await assertTaskReady(root, name);
} catch (error) {
	console.error(error instanceof Error ? error.message : error);
	process.exit(1);
}

await task(root, controller.signal, args);

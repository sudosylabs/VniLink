/**
 * Prepares a checkout in any state for a launch, after VS Code's
 * `build/lib/preLaunch.ts`: installs the dependencies when `node_modules` is
 * missing, and builds when the staged native addon is missing or older than
 * its sources. The task it precedes then checks what it needs and prepares
 * the branded Electron itself. `scripts/code.*`, `scripts/test.*`,
 * `npm run dev` and `npm run test:ui` run it first; `VNILINK_SKIP_PRELAUNCH=1`
 * skips it.
 *
 * Imports only Node's own modules (`native.ts` does too): it runs before the
 * dependencies are installed.
 */
import { spawn } from 'node:child_process';
import { access } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { assertNativeFresh } from './native.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

function runProcess(command: string, args: readonly string[]): Promise<void> {
	return new Promise((resolve, reject) => {
		// npm is a batch file on Windows, which only a shell starts.
		const child = spawn(command, args, { cwd: root, stdio: 'inherit', shell: process.platform === 'win32' && command === 'npm' });
		child.once('error', reject);
		child.once('exit', code => {
			if (code === 0) {
				resolve();
			} else {
				reject(new Error(`${command} ${args.join(' ')} exited with ${String(code)}`));
			}
		});
	});
}

export interface PreLaunchOptions {
	readonly root?: string;
	readonly environment?: NodeJS.ProcessEnv;
	readonly exists?: (path: string) => Promise<boolean>;
	/** Rejects when the staged addon is missing or stale; `native.ts`'s by default. */
	readonly assertNativeFresh?: (root: string) => Promise<void>;
	readonly run?: (command: string, args: readonly string[]) => Promise<void>;
}

export async function preLaunch(options: PreLaunchOptions = {}): Promise<void> {
	const { root: checkout = root, environment = process.env, run = runProcess } = options;
	if (environment.VNILINK_SKIP_PRELAUNCH) {
		return;
	}
	const exists = options.exists ?? (path => access(path).then(() => true, () => false));
	if (!await exists(join(checkout, 'node_modules'))) {
		console.log('[preLaunch] Installing dependencies (npm ci)');
		await run('npm', ['ci']);
	}
	try {
		await (options.assertNativeFresh ?? assertNativeFresh)(checkout);
		return;
	} catch {
		console.log('[preLaunch] The native addon is missing or older than its sources; building (node build/run.ts build)');
	}
	await run(process.execPath, [join(checkout, 'build/run.ts'), 'build']);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	preLaunch().catch((error: unknown) => {
		console.error(error instanceof Error ? error.message : error);
		process.exit(1);
	});
}

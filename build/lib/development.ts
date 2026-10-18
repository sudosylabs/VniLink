/** Supervises development watchers, typecheck workers and Electron restarts as one lifetime. */
import { createRequire } from 'node:module';
import { electronExecutable } from './electron.ts';
import { assertNativeFresh } from './native.ts';
import { ProcessRunner } from './processes.ts';
import { ApplicationWatcher } from './watch.ts';

const require = createRequire(import.meta.url);

/** Electron exits with this code to ask for a relaunch; see `lifecycleMainService.ts`. */
const DEVELOPMENT_RELAUNCH_EXIT_CODE = 42;

export class DevelopmentSession {
	readonly #root: string;
	readonly #args: readonly string[];
	readonly #processes = new ProcessRunner();
	readonly #signal: AbortSignal;
	#executable: string | undefined;
	readonly #watcher: ApplicationWatcher;
	#stopping: Promise<void> | undefined;
	#closed = false;
	readonly #stopped = Promise.withResolvers<undefined>();

	constructor(root: string, args: readonly string[], signal: AbortSignal) {
		this.#root = root;
		this.#args = args;
		this.#signal = signal;
		this.#watcher = new ApplicationWatcher(root, error => {
			console.error(error);
			void this.stop(1);
		});
		signal.addEventListener('abort', () => {
			void this.stop();
		}, { once: true });
	}

	/** Resolves once the session has stopped. */
	public async run(): Promise<void> {
		try {
			const watchOnly = this.#args.includes('--watch-only');
			if (!watchOnly) {
				await assertNativeFresh(this.#root);
				this.#executable = await electronExecutable(this.#root, this.#signal);
			}
			await this.#watcher.start();
			if (this.#closed) {
				return;
			}
			const tsc = require.resolve('typescript/bin/tsc');
			this.#worker(process.execPath, [tsc, '--noEmit', '--watch', '--preserveWatchOutput']);
			this.#worker(process.execPath, [tsc, '-p', 'src/tsconfig.json', '--noEmit', '--watch', '--preserveWatchOutput']);
			if (!watchOnly) {
				this.#electron();
			}
		} catch (error) {
			console.error('[dev]', error);
			await this.stop(1);
		}
		await this.#stopped.promise;
	}

	#worker(command: string, args: readonly string[]): void {
		const launched = this.#processes.start(command, args, { cwd: this.#root });
		launched.result.then(() => this.stop(0), (error: unknown) => {
			if (!this.#closed) {
				console.error(error);
				return this.stop(1);
			}
			return undefined;
		}).catch(() => undefined);
	}

	#electron(): void {
		if (this.#closed || !this.#executable) {
			return;
		}
		const launched = this.#processes.start(this.#executable, [this.#root, ...this.#args], { cwd: this.#root, env: { ...process.env, VNILINK_DEV: '1' } });
		launched.child.once('exit', code => {
			if (this.#closed) {
				return;
			}
			if (code === DEVELOPMENT_RELAUNCH_EXIT_CODE) {
				this.#electron();
			} else {
				void this.stop(code ?? 1);
			}
		});
		launched.result.catch((error: unknown) => {
			if (!this.#closed && launched.child.exitCode !== DEVELOPMENT_RELAUNCH_EXIT_CODE) {
				console.error(error);
				return this.stop(1);
			}
			return undefined;
		});
	}

	public stop(code = 0): Promise<void> {
		this.#closed = true;
		this.#stopping ??= Promise.allSettled([this.#watcher.dispose(), this.#processes.stop()]).then(results => {
			process.exitCode = results.some(result => result.status === 'rejected') ? 1 : code;
			this.#stopped.resolve(undefined);
		});
		return this.#stopping;
	}
}

/** Launches the built application once, after checking the staged addon is current. */
export async function startApplication(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const processes = new ProcessRunner(signal);
	try {
		await assertNativeFresh(root);
		await processes.run(await electronExecutable(root, signal), [root, ...args], { cwd: root });
	} finally {
		await processes.stop();
	}
}

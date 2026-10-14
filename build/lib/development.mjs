/** Supervises development watchers, typecheck workers and Electron restarts as one lifetime. */
import { createRequire } from 'node:module';
import electron from 'electron';
import { ApplicationWatcher } from '../watch.mjs';
import { NativeBuild } from './nativeBuild.mjs';
import { ProcessRunner } from './processes.mjs';

export class DevelopmentSession {
	#stopping;
	#closed = false;

	constructor(root, args = []) {
		this.root = root;
		this.args = args;
		this.processes = new ProcessRunner();
		this.watcher = new ApplicationWatcher(root, error => {
			console.error(error);
			void this.stop(1);
		});
	}

	async start() {
		try {
			const watchOnly = this.args.includes('--watch-only');
			if (!watchOnly) {
				await new NativeBuild(this.root, this.processes).assertFresh();
			}
			await this.watcher.start();
			if (this.#closed) {
				return;
			}
			const tsc = createRequire(import.meta.url).resolve('typescript/bin/tsc');
			this.#worker(process.execPath, [tsc, '--noEmit', '--watch', '--preserveWatchOutput']);
			this.#worker(process.execPath, [tsc, '-p', 'src/tsconfig.json', '--noEmit', '--watch', '--preserveWatchOutput']);
			if (!watchOnly) {
				this.#electron();
			}
		} catch (error) {
			console.error('[dev]', error);
			await this.stop(1);
		}
	}

	#worker(command, args, options = {}) {
		const launched = this.processes.start(command, args, { cwd: this.root, ...options });
		void launched.result.then(() => this.stop(0), error => {
			if (!this.#closed) {
				console.error(error);
				return this.stop(1);
			}
		});
	}

	#electron() {
		if (this.#closed) {
			return;
		}
		const launched = this.processes.start(electron, [this.root, ...this.args], { cwd: this.root, env: { ...process.env, VNILINK_DEV: '1' } });
		launched.child.once('exit', code => {
			if (this.#closed) {
				return;
			}
			if (code === 42) {
				this.#electron();
			} else {
				void this.stop(code ?? 1);
			}
		});
		void launched.result.catch(error => {
			if (!this.#closed && launched.child.exitCode !== 42) {
				console.error(error);
				return this.stop(1);
			}
		});
	}

	stop(code = 0) {
		this.#closed = true;
		this.#stopping ??= Promise.allSettled([this.watcher.dispose(), this.processes.stop()]).then(results => {
			process.exitCode = results.some(result => result.status === 'rejected') ? 1 : code;
		});
		return this.#stopping;
	}
}

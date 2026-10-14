/** Owns launched process groups, failure propagation and bounded teardown on each host OS. */
import { spawn } from 'node:child_process';

export class ProcessRunner {
	#children = new Map();
	#stopping;
	#closed = false;

	start(command, args, options = {}) {
		if (this.#closed) {
			throw new Error('Process runner is closed');
		}
		const child = spawn(command, args, { stdio: 'inherit', ...options, detached: process.platform !== 'win32' });
		const result = new Promise((resolve, reject) => {
			child.once('error', error => {
				this.#children.delete(child);
				reject(error);
			});
			child.once('exit', (code, signal) => {
				this.#children.delete(child);
				try {
					if (process.platform !== 'win32' && child.pid) {
						this.killGroup(child.pid, 'SIGKILL');
					}
				} catch (error) {
					reject(error);
					return;
				}
				if (code === 0) {
					resolve(code);
				} else {
					reject(new Error(`${command} exited (${signal ?? code})`));
				}
			});
		});
		// Supervisors can observe failure later without an unhandled rejection in the meantime.
		void result.catch(() => undefined);
		this.#children.set(child, result);
		return { child, result };
	}

	async run(command, args, options = {}) {
		return await this.start(command, args, options).result;
	}

	killGroup(pid, signal) {
		try {
			process.kill(-pid, signal);
		} catch (error) {
			if (error.code !== 'ESRCH') {
				throw error;
			}
		}
	}

	stop() {
		this.#closed = true;
		this.#stopping ??= Promise.all([...this.#children].map(async ([child, result]) => {
			if (!child.pid) {
				await result.catch(() => undefined);
				return;
			}
			if (process.platform === 'win32') {
				await new Promise((resolve, reject) => {
					const killer = spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
					const timer = setTimeout(() => {
						killer.kill();
						reject(new Error('Process tree teardown timed out'));
					}, 5000);
					killer.once('close', () => clearTimeout(timer));
					killer.once('error', reject);
					killer.once('exit', code => code === 0 || child.exitCode !== null ? resolve() : reject(new Error('Process tree teardown failed')));
				});
			} else {
				const kill = signal => this.killGroup(child.pid, signal);
				kill('SIGTERM');
				let timer;
				const forced = new Promise((resolve, reject) => {
					timer = setTimeout(() => {
						try {
							kill('SIGKILL');
							resolve();
						} catch (error) {
							reject(error);
						}
					}, 3000);
				});
				try {
					await Promise.race([result.catch(() => undefined), forced]);
				} finally {
					clearTimeout(timer);
					// The exit listener retires remaining descendants. A second
					// group kill here races macOS reaping an already-dead group.
				}
			}
			await result.catch(() => undefined);
		}));
		return this.#stopping;
	}
}

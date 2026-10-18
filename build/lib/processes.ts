/** Owns launched process groups, failure propagation and bounded teardown on each host OS. */
import { spawn, type ChildProcess, type SpawnOptions } from 'node:child_process';

export interface LaunchedProcess {
	readonly child: ChildProcess;
	readonly result: Promise<number>;
}

function asError(error: unknown): Error {
	return error instanceof Error ? error : new Error(String(error));
}

function killGroup(pid: number, signal: NodeJS.Signals): void {
	try {
		process.kill(-pid, signal);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== 'ESRCH') {
			throw error;
		}
	}
}

export class ProcessRunner {
	readonly #children = new Map<ChildProcess, Promise<number>>();
	#stopping: Promise<void> | undefined;
	#closed = false;

	/** Stops every process when `signal` aborts. */
	constructor(signal?: AbortSignal) {
		signal?.addEventListener('abort', () => {
			void this.stop();
		}, { once: true });
	}

	public start(command: string, args: readonly string[], options: SpawnOptions = {}): LaunchedProcess {
		if (this.#closed) {
			throw new Error('Process runner is closed');
		}
		const child = spawn(command, args, { stdio: 'inherit', ...options, detached: process.platform !== 'win32' });
		const result = new Promise<number>((resolve, reject) => {
			child.once('error', error => {
				this.#children.delete(child);
				reject(error);
			});
			child.once('exit', (code, signal) => {
				this.#children.delete(child);
				try {
					if (process.platform !== 'win32' && child.pid) {
						killGroup(child.pid, 'SIGKILL');
					}
				} catch (error) {
					reject(asError(error));
					return;
				}
				if (code === 0) {
					resolve(code);
				} else {
					reject(new Error(`${command} exited (${String(signal ?? code)})`));
				}
			});
		});
		// Supervisors can observe failure later without an unhandled rejection in the meantime.
		result.catch(() => undefined);
		this.#children.set(child, result);
		return { child, result };
	}

	public run(command: string, args: readonly string[], options: SpawnOptions = {}): Promise<number> {
		return this.start(command, args, options).result;
	}

	public stop(): Promise<void> {
		this.#closed = true;
		this.#stopping ??= Promise.all([...this.#children].map(([child, result]) => terminate(child, result))).then(() => undefined);
		return this.#stopping;
	}
}

async function terminate(child: ChildProcess, result: Promise<number>): Promise<void> {
	const pid = child.pid;
	if (!pid) {
		await result.catch(() => undefined);
		return;
	}
	if (process.platform === 'win32') {
		await new Promise<void>((resolve, reject) => {
			const killer = spawn('taskkill', ['/pid', String(pid), '/T', '/F'], { stdio: 'ignore' });
			const timer = setTimeout(() => {
				killer.kill();
				reject(new Error('Process tree teardown timed out'));
			}, 5000);
			killer.once('close', () => {
				clearTimeout(timer);
			});
			killer.once('error', reject);
			killer.once('exit', code => {
				if (code === 0 || child.exitCode !== null) {
					resolve();
				} else {
					reject(new Error('Process tree teardown failed'));
				}
			});
		});
	} else {
		killGroup(pid, 'SIGTERM');
		let timer: NodeJS.Timeout | undefined;
		const forced = new Promise<void>((resolve, reject) => {
			timer = setTimeout(() => {
				try {
					killGroup(pid, 'SIGKILL');
					resolve();
				} catch (error) {
					reject(asError(error));
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
}

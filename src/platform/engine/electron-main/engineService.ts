import { utilityProcess } from 'electron';
import type { UtilityProcess } from 'electron';
import { join } from 'node:path';
import type { EngineCommand, EngineReply } from '../common/engine.js';
import { isEngineCommand, isEngineReply, record } from '../common/engine.js';
export class EngineService {
	private child: UtilityProcess | undefined;
	private connected = false;
	private started = false;
	private closing = false;
	private queue: Promise<void> = Promise.resolve();
	private pending = 0;
	private nextId = 0;
	private reply: { id: number; resolve(value: EngineReply): void; reject(error: Error): void } | undefined;
	public get available(): boolean {
		return this.connected && !this.closing;
	}
	public async start(temporary = false): Promise<void> {
		if (this.started || this.closing) {
			throw new Error('Engine already started');
		}
		this.started = true;
		const child = utilityProcess.fork(join(__dirname, 'engine.cjs'), temporary ? ['--temporary'] : [], { serviceName: 'VniLink Engine', stdio: 'ignore' });
		this.child = child;
		await new Promise<void>((resolve, reject) => {
			const timeout = setTimeout(() => {
				this.fail(); reject(new Error('Engine startup timed out'));
			}, 15000);
			child.on('message', (message: unknown) => {
				if (this.child !== child) {
					return;
				}
				if (!record(message)) {
					this.fail(); return;
				}
				if ('ready' in message) {
					clearTimeout(timeout);
					if (message.ready === true && !this.connected) {
						this.connected = true; resolve();
					} else {
						this.fail(); reject(new Error('Engine could not open its secure installation'));
					}
					return;
				}
				if (!this.reply || message.id !== this.reply.id) {
					this.fail(); return;
				}
				const pending = this.reply; this.reply = undefined;
				if (isEngineReply(message.reply)) {
					pending.resolve(message.reply);
				} else if (typeof message.error === 'string') {
					pending.reject(new Error('Engine operation failed'));
				} else {
					pending.reject(new Error('Invalid engine reply')); this.fail();
				}
			});
			child.once('exit', () => {
				clearTimeout(timeout); this.fail(); reject(new Error('Engine exited'));
			});
		});
	}
	public execute(command: EngineCommand): Promise<EngineReply> {
		if (!isEngineCommand(command)) {
			return Promise.reject(new Error('Invalid engine command'));
		}
		return this.enqueue(command);
	}
	private enqueue(command: EngineCommand | { type: 'shutdown' }): Promise<EngineReply> {
		if (!this.connected || this.pending >= 64 || (this.closing && command.type !== 'shutdown')) {
			return Promise.reject(new Error('Engine unavailable'));
		}
		this.pending++;
		const result = this.queue.then(() => this.send(command));
		this.queue = result.then(() => {
			this.pending--;
		}, () => {
			this.pending--;
		});
		return result;
	}
	private send(command: EngineCommand | { type: 'shutdown' }): Promise<EngineReply> {
		if (!this.child || !this.connected || (this.closing && command.type !== 'shutdown')) {
			return Promise.reject(new Error('Engine unavailable'));
		}
		const child = this.child;
		return new Promise((resolve, reject) => {
			const id = ++this.nextId;
			const timeout = setTimeout(() => {
				this.fail(); reject(new Error('Engine operation timed out'));
			}, 10000);
			this.reply = {
				id, resolve: value => {
					clearTimeout(timeout); resolve(value);
				}, reject: error => {
					clearTimeout(timeout); reject(error);
				}
			};
			try {
				child.postMessage({ id, command });
			} catch {
				clearTimeout(timeout); this.fail(); reject(new Error('Engine unavailable'));
			}
		});
	}
	public async stop(): Promise<void> {
		if (this.closing) {
			return;
		}
		this.closing = true;
		try {
			if (this.connected) {
				await this.enqueue({ type: 'shutdown' });
			}
		} finally {
			this.fail();
		}
	}
	private fail(): void {
		this.connected = false;
		const pending = this.reply; this.reply = undefined;
		pending?.reject(new Error('Engine unavailable'));
		const child = this.child; this.child = undefined;
		child?.kill();
	}
}

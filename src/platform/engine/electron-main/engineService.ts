import { utilityProcess } from 'electron';
import type { DataChannelRequest } from '../common/dataChannel.js';
import type { MessagePortMain, UtilityProcess } from 'electron';
import { join } from 'node:path';
import type { EngineCommand, EngineReply } from '../common/engine.js';
import { isEngineReply, record } from '../common/engine.js';

interface PendingReply { resolve(value: EngineReply): void; reject(error: Error): void; }

function timeoutFor(command: EngineCommand | { type: 'shutdown' }): number {
	// Core dials block up to 75 seconds; every other call answers quickly.
	return command.type === 'connect' || command.type === 'connectFiles' ? 90000 : 10000;
}

export class EngineService {
	private child: UtilityProcess | undefined;
	private connected = false;
	private started = false;
	private closing = false;
	private fastQueue: Promise<void> = Promise.resolve();
	private slowQueue: Promise<void> = Promise.resolve();
	private pending = 0;
	private nextId = 0;
	private waiting = new Map<number, PendingReply>();
	private inflightSnapshot: Promise<EngineReply> | undefined;

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
				this.fail(new Error('Engine startup timed out')); reject(new Error('Engine startup timed out'));
			}, 15000);
			child.on('message', (message: unknown) => {
				if (this.child !== child) {
					return;
				}
				if (!record(message)) {
					return;
				}
				if ('ready' in message) {
					clearTimeout(timeout);
					if (message.ready === true && !this.connected) {
						this.connected = true; resolve();
					} else {
						this.fail(new Error('Engine could not open its secure installation')); reject(new Error('Engine could not open its secure installation'));
					}
					return;
				}
				this.deliver(message);
			});
			child.once('exit', () => {
				clearTimeout(timeout); this.fail(new Error('Engine exited')); reject(new Error('Engine exited'));
			});
		});
	}
	public execute(command: EngineCommand): Promise<EngineReply> {
		// Validated at the IPC boundary and in the Rust bridge; this queue
		// stays typed-only so slow pairing work cannot hide a second check.
		if (command.type === 'snapshot') {
			const running = this.inflightSnapshot;
			if (running) {
				return running;
			}
			const coalesced = this.enqueue(command, this.fastQueue, queue => {
				this.fastQueue = queue;
			});
			this.inflightSnapshot = coalesced;
			const clear = (): void => {
				if (this.inflightSnapshot === coalesced) {
					this.inflightSnapshot = undefined;
				}
			};
			coalesced.then(clear, clear);
			return coalesced;
		}
		return this.enqueue(command, this.slowQueue, queue => {
			this.slowQueue = queue;
		});
	}
	private enqueue(command: EngineCommand | { type: 'shutdown' }, lane: Promise<void>, advance: (queue: Promise<void>) => void): Promise<EngineReply> {
		if (!this.connected || this.pending >= 64 || (this.closing && command.type !== 'shutdown')) {
			return Promise.reject(new Error('Engine unavailable'));
		}
		this.pending++;
		const result = lane.then(() => this.send(command));
		const tracked = result.then(value => {
			this.pending--;
			return value;
		}, (error: unknown) => {
			this.pending--;
			throw error;
		});
		advance(tracked.then(() => undefined, () => undefined));
		return tracked;
	}
	private deliver(message: Record<string, unknown>): void {
		if (typeof message.id !== 'number' || !Number.isSafeInteger(message.id)) {
			return;
		}
		const pending = this.waiting.get(message.id);
		if (!pending) {
			return;
		}
		this.waiting.delete(message.id);
		if (isEngineReply(message.reply)) {
			pending.resolve(message.reply);
		} else if (typeof message.error === 'string' && message.error.length > 0) {
			pending.reject(new Error(message.error.slice(0, 512)));
		} else {
			pending.reject(new Error('Invalid engine reply'));
		}
	}
	private send(command: EngineCommand | { type: 'shutdown' }): Promise<EngineReply> {
		if (!this.child || !this.connected || (this.closing && command.type !== 'shutdown')) {
			return Promise.reject(new Error('Engine unavailable'));
		}
		const child = this.child;
		return new Promise((resolve, reject) => {
			const id = ++this.nextId;
			const timeout = setTimeout(() => {
				// A slow call fails alone; the engine stays up for the rest.
				this.waiting.delete(id);
				reject(new Error('Engine operation timed out'));
			}, timeoutFor(command));
			this.waiting.set(id, {
				resolve: value => {
					clearTimeout(timeout); resolve(value);
				}, reject: error => {
					clearTimeout(timeout); reject(error);
				}
			});
			try {
				child.postMessage({ id, command });
			} catch {
				clearTimeout(timeout);
				this.waiting.delete(id);
				reject(new Error('Engine unavailable'));
			}
		});
	}
	public attachChannel(port: MessagePortMain, request: DataChannelRequest, owner: string): void {
		if (!this.child || !this.available) {
			port.close(); throw new Error('Engine unavailable');
		}
		this.child.postMessage({ type: 'attachDataChannel', request, owner }, [port]);
	}
	public closeChannels(owner: string): void {
		this.child?.postMessage({ type: 'closeDataChannels', owner });
	}
	public async stop(): Promise<void> {
		if (this.closing) {
			return;
		}
		this.closing = true;
		try {
			if (this.connected) {
				await this.enqueue({ type: 'shutdown' }, this.slowQueue, queue => {
					this.slowQueue = queue;
				});
			}
		} finally {
			this.fail(new Error('Engine unavailable'));
		}
	}
	private fail(error: Error): void {
		this.connected = false;
		for (const pending of this.waiting.values()) {
			pending.reject(error);
		}
		this.waiting.clear();
		const child = this.child; this.child = undefined;
		child?.kill();
	}
}

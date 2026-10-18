import { MessageChannelMain, utilityProcess } from 'electron';
import type { MessagePortMain, UtilityProcess } from 'electron';
import { join } from 'node:path';
import { CancellationTokenSource } from '../../../base/common/cancellation.js';
import { Emitter, Event, Relay } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';
import { Client as MessagePortClient } from '../../../base/parts/ipc/electron-main/ipc.mp.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import type { DataChannelRequest } from '../common/dataChannel.js';
import type { EngineCommand, EngineReply } from '../common/engine.js';
import { isEngineEvent, isEngineReply, record } from '../common/engine.js';
import { ENGINE_HOST_CHANNEL_NAME, type EngineEventBatch } from '../common/engineService.js';

export const IEngineMainService = createDecorator<EngineMainService>('engineMainService');

/** The event stream while no engine is connected. */
const noEvents: Event<EngineEventBatch> = () => Disposable.None;

/** At most this many engine requests may wait at once. */
const maxPendingRequests = 64;

function timeoutFor(command: EngineCommand | { type: 'shutdown' }): number {
	// Core dials block up to 75 seconds; every other call answers quickly.
	if (command.type === 'shutdown' || command.type === 'startNativeScreenSharing') {
		return 30000;
	}
	return command.type === 'connect' || command.type === 'connectFiles' ? 90000 : 10000;
}

/** Owns the engine utility process, its request lanes and its lifetime. */
export class EngineMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly availabilityChanged = this._register(new Emitter<boolean>());
	public readonly onDidChangeAvailability = this.availabilityChanged.event;
	private readonly restartRequiredChanged = this._register(new Emitter<boolean>());
	public readonly onDidChangeRestartRequired = this.restartRequiredChanged.event;
	private readonly events = this._register(new Relay<EngineEventBatch>());
	/** Engine events, validated here; the host subscribes only while this has listeners. */
	public readonly onDidReceiveEvents = this.events.event;
	private restartRequired = false;
	private child: UtilityProcess | undefined;
	private client: MessagePortClient | undefined;
	private channel: IChannel | undefined;
	private connected = false;
	private started = false;
	private closing = false;
	private fastQueue: Promise<void> = Promise.resolve();
	private slowQueue: Promise<void> = Promise.resolve();
	private pending = 0;
	private readonly waiting = new Set<(error: Error) => void>();
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
				this.fail(new Error('Engine startup timed out'));
				reject(new Error('Engine startup timed out'));
			}, 60_000);
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
						this.connect(child);
						this.connected = true;
						this.availabilityChanged.fire(true);
						resolve();
					} else {
						const reason = typeof message.error === 'string' ? `: ${message.error}` : '';
						const error = new Error(`Engine could not open its secure installation${reason}`);
						this.fail(error);
						reject(error);
					}
				}
			});
			child.once('exit', code => {
				clearTimeout(timeout);
				const error = new Error(`Engine exited (${String(code)})`);
				this.fail(error);
				reject(error);
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
		if (!this.connected || this.pending >= maxPendingRequests || (this.closing && command.type !== 'shutdown')) {
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

	/** Opens the IPC connection to the host over a fresh `MessagePort`. */
	private connect(child: UtilityProcess): void {
		const { port1, port2 } = new MessageChannelMain();
		child.postMessage('vscode:engineHost', [port2]);
		this.client = new MessagePortClient(port1, 'main');
		this.channel = this.client.getChannel(ENGINE_HOST_CHANNEL_NAME);
		this.events.input = Event.map(this.channel.listen<unknown>('onDidReceiveEvents'), batch => validEventBatch(batch));
	}

	private send(command: EngineCommand | { type: 'shutdown' }): Promise<EngineReply> {
		const channel = this.channel;
		if (!channel || !this.connected || (this.closing && command.type !== 'shutdown')) {
			return Promise.reject(new Error('Engine unavailable'));
		}
		return new Promise((resolve, reject) => {
			const cancellation = new CancellationTokenSource();
			const settle = (): void => {
				clearTimeout(timeout);
				this.waiting.delete(fail);
				cancellation.dispose();
			};
			const fail = (error: Error): void => {
				settle();
				reject(error);
			};
			const timeout = setTimeout(() => {
				// A slow call fails alone; the engine stays up for the rest.
				cancellation.cancel();
				fail(new Error('Engine operation timed out'));
			}, timeoutFor(command));
			this.waiting.add(fail);
			channel.call<unknown>('execute', [command], cancellation.token).then(reply => {
				if (!this.waiting.has(fail)) {
					return;
				}
				settle();
				if (isEngineReply(reply)) {
					resolve(reply);
				} else {
					reject(new Error('Invalid engine reply'));
				}
			}, (error: unknown) => {
				if (this.waiting.has(fail)) {
					fail(error instanceof Error && error.message ? new Error(error.message.slice(0, 512)) : new Error('Engine operation failed'));
				}
			});
		});
	}

	/** Whether saved relay settings differ from the ones the running engine uses. */
	public async getRestartRequired(): Promise<boolean> {
		const reply = await this.execute({ type: 'relayRestartRequired' });
		if (reply.type !== 'restartRequired') {
			throw new Error('Invalid restart state');
		}
		return reply.required;
	}

	/** Re-reads the restart requirement and announces a change. */
	public async refreshRestartRequired(): Promise<void> {
		const required = await this.getRestartRequired();
		if (required !== this.restartRequired) {
			this.restartRequired = required;
			this.restartRequiredChanged.fire(required);
		}
	}

	public attachChannel(port: MessagePortMain, request: DataChannelRequest, owner: string): void {
		if (!this.child || !this.available) {
			port.close();
			throw new Error('Engine unavailable');
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
		const wasAvailable = this.available;
		this.connected = false;
		for (const reject of [...this.waiting]) {
			reject(error);
		}
		this.events.input = noEvents;
		this.channel = undefined;
		this.client?.dispose();
		this.client = undefined;
		const child = this.child;
		this.child = undefined;
		child?.kill();
		if (wasAvailable) {
			this.availabilityChanged.fire(false);
		}
	}
}

/** Keeps only well-formed events; anything dropped is reported as a resync. */
function validEventBatch(value: unknown): EngineEventBatch {
	if (!record(value) || !Array.isArray(value.events)) {
		return { events: [], resync: true };
	}
	const events = value.events.filter(isEngineEvent);
	return { events, resync: value.resync === true || events.length !== value.events.length };
}

import { isDesktopAudioOperation, isDesktopAudioStatus, isDataPacket, isChannelRequest, MAX_FILE_OPERATIONS } from '../common/dataChannel.js';
import type { DataChannelRequest, DataPacket, DataChannelMetrics, DesktopAudioOperation, DesktopAudioStatus } from '../common/dataChannel.js';
import { isFileAnswer, type FileAnswer, type FileOperation } from '../common/files.js';
import { ipcMessagePort } from '../../../base/parts/sandbox/electron-browser/globals.js';
import { record } from '../common/engine.js';
import type { EngineCommand } from '../common/engine.js';
import { DATA_CHANNEL_PORT_RESPONSE, type IEngineService } from '../common/engineService.js';

/** Owns the renderer end; payloads arrive outside contextBridge's copied values. */
export class DataChannelClient {
	private closed = false;
	private readonly inputQueue: EngineCommand[] = [];
	private inputFlight = false;
	private pointerTimer: ReturnType<typeof setTimeout> | undefined;
	private expectedSequence = 1;
	private lastHeard = performance.now();
	private readonly heartbeat: ReturnType<typeof setInterval>;
	public metrics: DataChannelMetrics = { sent: 0, acknowledged: 0, pending: 0, lastAckMs: 0 };
	private readonly pendingWrites = new Map<number, { resolve(): void; reject(error: Error): void }>();
	private nextWrite = 0;
	private audioPending: {
		id: number;
		resolve(value: DesktopAudioStatus): void;
		reject(error: Error): void;
		timer: ReturnType<typeof setTimeout>;
	} | undefined;
	private readonly filePending = new Map<number, { resolve(value: FileAnswer): void; reject(error: Error): void }>();

	constructor(
		private readonly port: MessagePort,
		private readonly consume: (packet: DataPacket) => void | Promise<void>,
		private readonly onError: (error: Error) => void,
	) {
		port.onmessage = (event) => {
			void this.receive(event.data);
		};
		port.onmessageerror = () => {
			this.fail(new Error('Invalid data-channel packet'));
		};
		port.start();
		this.heartbeat = setInterval(() => {
			if (performance.now() - this.lastHeard > 15000) {
				this.fail(new Error('Data channel disconnected'));
			} else {
				port.postMessage({ type: 'metrics' });
			}
		}, 2000);
	}

	public static async open(
		engine: IEngineService,
		request: Omit<DataChannelRequest, 'requestId'>,
		consume: (packet: DataPacket) => void | Promise<void>,
		onError: (error: Error) => void,
	): Promise<DataChannelClient> {
		const target = { ...request, requestId: crypto.randomUUID() };
		if (!isChannelRequest(target)) {
			throw new Error('Invalid data channel request');
		}
		return await new Promise<DataChannelClient>((resolve, reject) => {
			const timer = setTimeout(() => {
				cleanup();
				reject(new Error('Data channel startup timed out'));
			}, 15000);
			const cleanup = (): void => {
				clearTimeout(timer);
				window.removeEventListener('message', receive);
			};
			const receive = (event: MessageEvent<unknown>): void => {
				if (event.source !== window || event.origin !== location.origin || event.data !== target.requestId) {
					return;
				}
				const port = event.ports.at(0);
				if (!port) {
					cleanup();
					reject(new Error('Missing data port'));
					return;
				}
				cleanup();
				const timeout = setTimeout(() => {
					port.close();
					reject(new Error('Native channel startup timed out'));
				}, 15000);
				port.onmessage = (message) => {
					clearTimeout(timeout);
					if (record(message.data) && message.data.type === 'ready') {
						resolve(new DataChannelClient(port, consume, onError));
					} else {
						port.close();
						// The engine's category (Busy, Unauthorized…) says why it refused; the message stays generic.
						const category = record(message.data) && typeof message.data.category === 'string' && /^[A-Z][A-Za-z]{1,31}$/.test(message.data.category) ? message.data.category : undefined;
						reject(new Error(category ? `${category}: Native data channel unavailable` : 'Native data channel unavailable'));
					}
				};
				port.start();
			};
			window.addEventListener('message', receive);
			try {
				ipcMessagePort.acquire(DATA_CHANNEL_PORT_RESPONSE, target.requestId);
			} catch (error) {
				cleanup();
				reject(error instanceof Error ? error : new Error('Channel unavailable'));
				return;
			}
			void engine.openDataChannel(target).catch((error: unknown) => {
				cleanup();
				reject(error instanceof Error ? error : new Error('Channel unavailable'));
			});
		});
	}

	private async receive(message: unknown): Promise<void> {
		if (this.closed) {
			return;
		}
		this.lastHeard = performance.now();
		try {
			if (record(message) && message.type === 'metrics') {
				if (
					typeof message.sent !== 'number' ||
					typeof message.acknowledged !== 'number' ||
					typeof message.pending !== 'number' ||
					typeof message.lastAckMs !== 'number'
				) {
					throw new Error('Invalid metrics');
				}
				this.metrics = {
					sent: message.sent,
					acknowledged: message.acknowledged,
					pending: message.pending,
					lastAckMs: message.lastAckMs,
				};
				return;
			}
			if (record(message) && message.type === 'desktopAudio') {
				if (!Number.isSafeInteger(message.id) || typeof message.id !== 'number' || message.id <= 0 || !isDesktopAudioStatus(message.status)) {
					throw new Error('Invalid desktop audio status');
				}
				const pending = this.audioPending;
				if (pending?.id !== message.id) {
					return; // A timed-out request must not close a healthy desktop.
				}
				this.audioPending = undefined;
				clearTimeout(pending.timer);
				pending.resolve(message.status);
				return;
			}
			if (record(message) && message.type === 'closed') {
				this.close();
				return;
			}
			if (record(message) && message.type === 'error') {
				throw new Error('Data channel unavailable');
			}
			if (
				record(message) &&
				message.type === 'fileReply' &&
				typeof message.id === 'number' && this.filePending.has(message.id) &&
				typeof message.metadata === 'string'
			) {
				const answer: unknown = JSON.parse(message.metadata);
				if (!isFileAnswer(answer)) {
					throw new Error('Invalid file answer');
				}
				this.filePending.get(message.id)?.resolve(answer);
				this.filePending.delete(message.id);
				return;
			}
			if (record(message) && message.type === 'written' && typeof message.id === 'number') {
				this.pendingWrites.get(message.id)?.resolve();
				this.pendingWrites.delete(message.id);
				return;
			}
			if (!isDataPacket(message) || message.sequence !== this.expectedSequence++) {
				throw new Error('Invalid data sequence');
			}
			await this.consume(message);
			// Consumers may close the channel while an asynchronous write is pending.
			// eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
			if (!this.closed) {
				this.port.postMessage({ type: 'ack', sequence: message.sequence });
			}
		} catch (error) {
			this.fail(error instanceof Error ? error : new Error('Data consumer failed'));
		}
	}

	public input(command: EngineCommand): void {
		if (this.closed) {
			throw new Error('Data channel closed');
		}
		const tail = this.inputQueue.at(-1);
		if (command.type === 'submitPointer' && tail?.type === 'submitPointer') {
			this.inputQueue[this.inputQueue.length - 1] = command;
		} else {
			if (this.inputQueue.length >= 64) {
				this.fail(new Error('Input backpressure'));
				return;
			}
			this.inputQueue.push(command);
		}
		this.pointerTimer ??= setTimeout(() => {
			this.pointerTimer = undefined;
			this.flushInput();
		}, 0);
	}

	private flushInput(): void {
		if (this.closed || this.inputFlight) {
			return;
		}
		const command = this.inputQueue.shift();
		if (!command) {
			return;
		}
		this.inputFlight = true;
		const id = ++this.nextWrite;
		void new Promise<void>((resolve, reject) => {
			this.pendingWrites.set(id, { resolve, reject });
			this.port.postMessage({ type: 'input', id, command });
		}).then(
			() => {
				this.inputFlight = false;
				this.flushInput();
			},
			(error: unknown) => {
				this.fail(error instanceof Error ? error : new Error('Input failed'));
			},
		);
	}

	public async write(data: Uint8Array): Promise<void> {
		if (this.closed || data.length < 1 || data.length > 16384 || this.pendingWrites.size >= 16) {
			throw new Error('Terminal input backpressure');
		}
		const id = ++this.nextWrite;
		await new Promise<void>((resolve, reject) => {
			this.pendingWrites.set(id, { resolve, reject });
			this.port.postMessage({ type: 'write', id, data });
		});
	}

	/** Asks the other computer; resolves with its answer, which may be a refusal. Rejects only when the channel is gone or full. */
	public async fileOperation(operation: FileOperation): Promise<FileAnswer> {
		if (this.closed || this.filePending.size >= MAX_FILE_OPERATIONS) {
			throw new Error('File operation backpressure');
		}
		return await new Promise<FileAnswer>((resolve, reject) => {
			const id = ++this.nextWrite;
			this.filePending.set(id, { resolve, reject });
			this.port.postMessage({ type: 'file', id, operation });
		});
	}

	public async desktopAudio(operation: DesktopAudioOperation): Promise<DesktopAudioStatus> {
		if (this.closed || this.audioPending || !isDesktopAudioOperation(operation) || this.nextWrite >= Number.MAX_SAFE_INTEGER) {
			throw new Error('Desktop audio operation unavailable');
		}
		return await new Promise<DesktopAudioStatus>((resolve, reject) => {
			const id = ++this.nextWrite;
			const timer = setTimeout(() => {
				if (this.audioPending?.id === id) {
					this.audioPending = undefined;
					reject(new Error('Desktop audio operation timed out'));
				}
			}, 5000);
			this.audioPending = { id, resolve, reject, timer };
			try {
				this.port.postMessage({ type: 'desktopAudio', id, operation });
			} catch (error) {
				clearTimeout(timer);
				this.audioPending = undefined;
				reject(error instanceof Error ? error : new Error('Desktop audio message failed'));
			}
		});
	}

	public resize(rows: number, columns: number): void {
		if (!this.closed) {
			this.port.postMessage({ type: 'resize', rows, columns });
		}
	}

	private fail(error: Error): void {
		if (this.closed) {
			return;
		}
		this.close();
		this.onError(error);
	}

	public close(): void {
		if (this.closed) {
			return;
		}
		this.closed = true;
		clearInterval(this.heartbeat);
		if (this.audioPending) {
			clearTimeout(this.audioPending.timer);
			this.audioPending.reject(new Error('Data channel closed'));
			this.audioPending = undefined;
		}
		if (this.pointerTimer) {
			clearTimeout(this.pointerTimer);
		}
		this.inputQueue.length = 0;
		for (const pending of this.pendingWrites.values()) {
			pending.reject(new Error('Data channel closed'));
		}
		this.pendingWrites.clear();
		for (const pending of this.filePending.values()) {
			pending.reject(new Error('Data channel closed'));
		}
		this.filePending.clear();
		this.port.postMessage({ type: 'close' });
		this.port.close();
	}
}

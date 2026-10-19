import { Buffer } from 'node:buffer';
import { dimension, isInputCommand, isDesktopAudioOperation, isDesktopAudioStatus, MAX_FILE_OPERATIONS } from '../common/dataChannel.js';
import type { DataChannelRequest } from '../common/dataChannel.js';
import { isFileOperation } from '../common/files.js';
import { record } from '../common/engine.js';

export interface NativeDataChannel {
	desktopAudio(operation: string): string;
	read(): { kind: string; data?: Uint8Array | null; metadata: string } | null;
	write(data: Uint8Array): void;
	resize(rows: number, columns: number): void;
	input(command: string): void;
	close(): void | Promise<void>;
	fileOperation(command: string): Promise<string>;
}

export interface DataPort {
	postMessage(message: unknown): void;
	on(event: 'message', listener: (event: { data: unknown }) => void): void;
	on(event: 'close', listener: () => void): void;
	start(): void;
	close(): void;
}
/** One unacknowledged packet per channel. Native queues retain decode/terminal
 * order; the renderer acknowledges only after consuming each packet. */
export class DataChannelHost {
	private closed = false;
	private native: NativeDataChannel | undefined;
	private timer: ReturnType<typeof setTimeout> | undefined;
	private sequence = 0;
	private pending = 0;
	private ended = false;
	private sentAt = 0;
	private lastAckMs = 0;
	/** File operations answered by the engine but not yet replied. */
	private fileFlight = 0;

	constructor(
		private readonly port: DataPort,
		private readonly request: DataChannelRequest,
		private readonly onClose: () => void,
	) {
		port.on('message', (event) => {
			this.receive(event.data);
		});
		port.on('close', () => {
			this.close();
		});
		port.start();
	}

	public start(native: NativeDataChannel): void {
		if (this.closed) {
			const closing = native.close();
			if (closing) {
				void closing.catch(() => { /* Engine shutdown may already have retired this owner. */ });
			}
			return;
		}
		this.native = native;
		this.port.postMessage({ type: 'ready' });
		// A files channel reads too: the engine pushes its job changes.
		if (this.request.kind !== 'input') {
			this.pump();
		}
	}

	private receive(message: unknown): void {
		if (this.closed || !record(message)) {
			return;
		}
		try {
			if (message.type === 'close') {
				this.close();
				return;
			}
			if (!this.native) {
				throw new Error('Channel not ready');
			}
			if (message.type === 'ack' && message.sequence === this.pending && this.pending > 0) {
				this.lastAckMs = performance.now() - this.sentAt;
				this.pending = 0;
				if (this.ended) {
					this.close();
				} else {
					this.pump();
				}
				return;
			}
			if (message.type === 'desktopAudio' && (this.request.kind === 'video' || this.request.kind === 'input') && Number.isSafeInteger(message.id) && Number(message.id) > 0 && isDesktopAudioOperation(message.operation)) {
				const status: unknown = JSON.parse(this.native.desktopAudio(JSON.stringify(message.operation)));
				if (!isDesktopAudioStatus(status)) {
					throw new Error('Invalid desktop audio status');
				}
				this.port.postMessage({ type: 'desktopAudio', id: message.id, status });
				return;
			}
			if (message.type === 'metrics') {
				this.port.postMessage({
					type: 'metrics',
					sent: this.sequence,
					acknowledged: this.sequence - Number(this.pending > 0),
					pending: Number(this.pending > 0),
					lastAckMs: this.lastAckMs,
				});
				return;
			}
			if (
				message.type === 'input' &&
				this.request.kind === 'input' &&
				isInputCommand(message.command) &&
				'sessionId' in message.command &&
				message.command.sessionId === this.request.sessionId &&
				'generation' in message.command &&
				message.command.generation === this.request.generation
			) {
				this.native.input(JSON.stringify(message.command));
				if (Number.isSafeInteger(message.id)) {
					this.port.postMessage({ type: 'written', id: message.id });
				}
				return;
			}
			if (
				message.type === 'write' &&
				this.request.kind === 'terminal' &&
				message.data instanceof Uint8Array &&
				message.data.length > 0 &&
				message.data.length <= 16384
			) {
				this.native.write(Buffer.from(message.data));
				if (Number.isSafeInteger(message.id)) {
					this.port.postMessage({ type: 'written', id: message.id });
				}
				return;
			}
			if (
				message.type === 'file' &&
				this.request.kind === 'files' &&
				isFileOperation(message.operation) &&
				this.fileFlight < MAX_FILE_OPERATIONS &&
				Number.isSafeInteger(message.id)
			) {
				const command = JSON.stringify(message.operation);
				if (command.length > 16384) {
					throw new Error('File operation too large');
				}
				this.fileFlight++;
				// A refused operation is an answer; only a lost session rejects.
				void this.native.fileOperation(command).then(
					(metadata) => {
						this.fileFlight--;
						if (!this.closed) {
							this.port.postMessage({ type: 'fileReply', id: message.id, metadata });
						}
					},
					(error: unknown) => {
						this.fileFlight--;
						this.fail(undefined, error instanceof Error ? error.message : undefined);
					},
				);
				return;
			}
			if (
				message.type === 'resize' &&
				this.request.kind === 'terminal' &&
				dimension(message.rows) &&
				dimension(message.columns)
			) {
				this.native.resize(message.rows, message.columns);
				return;
			}
			throw new Error('Invalid data-channel message');
		} catch (error) {
			this.fail(undefined, error instanceof Error ? error.message : undefined);
		}
	}

	private pump(): void {
		if (
			this.closed ||
			this.pending ||
			!this.native ||
			this.request.kind === 'input'
		) {
			return;
		}
		if (this.timer) {
			clearTimeout(this.timer);
			this.timer = undefined;
		}
		try {
			const packet = this.native.read();
			if (packet) {
				this.ended = packet.kind === 'exited' || packet.kind === 'failed';
				this.pending = ++this.sequence;
				this.sentAt = performance.now();
				this.port.postMessage({
					type: 'data',
					sequence: this.pending,
					kind: packet.kind,
					data: packet.data ?? undefined,
					metadata: packet.metadata,
				});
				// A stalled renderer must not retain a session indefinitely.
				this.timer = setTimeout(() => {
					this.fail(undefined, 'the window did not take a packet within 10 seconds');
				}, 10000);
			} else {
				this.timer = setTimeout(() => {
					this.pump();
				}, 4);
			}
		} catch (error) {
			this.fail(undefined, error instanceof Error ? error.message : undefined);
		}
	}

	/**
	 * Ends the channel. `category` is the engine's error category when it
	 * refused to open; `reason` says why a working channel ended, for the log.
	 */
	public fail(category?: string, reason?: string): void {
		if (this.closed) {
			return;
		}
		try {
			this.port.postMessage({ type: 'error', message: 'Data channel unavailable', category, reason: reason?.slice(0, 300) });
		} finally {
			this.close();
		}
	}

	public close(): void {
		if (this.closed) {
			return;
		}
		this.closed = true;
		if (this.timer) {
			clearTimeout(this.timer);
		}
		const closing = this.native?.close();
		if (closing) {
			void closing.catch(() => { /* Engine shutdown may already have retired this owner. */ });
		}
		this.native = undefined;
		try {
			this.port.postMessage({ type: 'closed' });
		} catch { /* The peer may have already closed its endpoint. */ }
		this.port.close();
		this.onClose();
	}
}

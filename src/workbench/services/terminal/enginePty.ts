import * as nls from '../../../nls.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { DataChannelRequest, DataPacket } from '../../../platform/engine/common/dataChannel.js';
import type { RemoteStatus } from '../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import { MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS, type IProcessDataEvent, type ITerminalChildProcess, type ITerminalLaunchError, type TerminalExit, type TerminalLaunchFailure } from '../../../platform/terminal/common/terminal.js';
import { formatMessageForTerminal } from '../../../platform/terminal/common/terminalStrings.js';
import { IDeviceService } from '../devices/deviceService.js';

/** The renderer end of a terminal data channel (`DataChannelClient`). */
export interface TerminalChannel {
	write(data: Uint8Array): Promise<void>;
	resize(rows: number, columns: number): void;
	close(): void;
}

export type TerminalChannelOpener = (
	request: Omit<DataChannelRequest, 'requestId'>,
	consume: (packet: DataPacket) => Promise<void>,
	onError: (error: Error) => void,
) => Promise<TerminalChannel>;

/** Largest write the data channel accepts. */
const chunkBytes = 16384;
/** Writes in flight; the channel refuses a seventeenth. */
const writesInFlight = 8;
/** Typed or pasted bytes held while the shell catches up. */
const pendingInputBytes = 1024 * 1024;

function clamp(value: number, max: number): number {
	return Number.isFinite(value) ? Math.min(max, Math.max(1, Math.floor(value))) : 1;
}

function launchFailure(status: RemoteStatus): TerminalLaunchFailure | undefined {
	switch (status) {
		case 'available': return undefined;
		case 'turnedOff':
		case 'needsPermission': return 'notAllowed';
		case 'unsupported': return 'unsupported';
		case 'unavailable': return 'unavailable';
	}
}

function exitCode(metadata: string): number {
	let parsed: unknown;
	try {
		parsed = JSON.parse(metadata);
	} catch {
		throw new Error('Invalid terminal exit');
	}
	const code: unknown = typeof parsed === 'object' && parsed !== null ? Reflect.get(parsed, 'code') : undefined;
	if (typeof code !== 'number' || !Number.isSafeInteger(code) || code < 0) {
		throw new Error('Invalid terminal exit');
	}
	return code;
}

/**
 * A shell on a paired computer over the engine's terminal data channel, as
 * VS Code's `AgentHostPty` is a shell over its own protocol. Flow control is
 * the channel's: the host sends the next packet only after the previous one
 * is acknowledged, and output is acknowledged once the emulator parsed it.
 */
export class EnginePty extends Disposable implements ITerminalChildProcess {
	private readonly dataEmitter = this._register(new Emitter<IProcessDataEvent>());
	public readonly onProcessData = this.dataEmitter.event;
	private readonly readyEmitter = this._register(new Emitter<void>());
	public readonly onProcessReady = this.readyEmitter.event;
	private readonly exitEmitter = this._register(new Emitter<TerminalExit>());
	public readonly onProcessExit = this.exitEmitter.event;
	private state: 'idle' | 'starting' | 'running' | 'ended' = 'idle';
	private channel: TerminalChannel | undefined;
	private cols: number;
	private rows: number;
	private readonly pending: Uint8Array[] = [];
	private pendingBytes = 0;
	private inFlight = 0;
	private releaseOutput: (() => void) | undefined;

	constructor(
		private readonly deviceId: string,
		cols: number,
		rows: number,
		private readonly openChannel: TerminalChannelOpener,
		@IEngineService private readonly engine: IEngineService,
		@IDeviceService private readonly devices: IDeviceService,
	) {
		super();
		this.cols = clamp(cols, MAX_TERMINAL_COLUMNS);
		this.rows = clamp(rows, MAX_TERMINAL_ROWS);
	}

	public async start(): Promise<ITerminalLaunchError | undefined> {
		if (this.state !== 'idle') {
			return { code: 'failed' };
		}
		this.state = 'starting';
		const refused = await this.refusal();
		if (!this.isStarting()) {
			return undefined;
		}
		if (refused) {
			this.state = 'ended';
			return { code: refused };
		}
		const session = await this.session();
		if (!this.isStarting()) {
			return undefined;
		}
		if (!session) {
			this.state = 'ended';
			return { code: 'disconnected' };
		}
		const size = { cols: this.cols, rows: this.rows };
		const opening = { lost: false };
		let channel: TerminalChannel;
		try {
			channel = await this.openChannel(
				{ kind: 'terminal', sessionId: session.id, generation: session.generation, rows: size.rows, columns: size.cols },
				packet => this.consume(packet),
				() => {
					opening.lost = this.isStarting();
					this.finish({ reason: 'lost' });
				},
			);
		} catch {
			if (!this.isStarting()) {
				return undefined;
			}
			this.state = 'ended';
			return { code: 'failed' };
		}
		if (!this.isStarting()) {
			channel.close();
			return undefined;
		}
		this.channel = channel;
		this.state = 'running';
		if (size.cols !== this.cols || size.rows !== this.rows) {
			channel.resize(this.rows, this.cols);
		}
		this.readyEmitter.fire();
		if (opening.lost) {
			this.finish({ reason: 'lost' });
		}
		this.flush();
		return undefined;
	}

	public input(data: string): void {
		this.send(new TextEncoder().encode(data));
	}

	public processBinary(data: string): void {
		this.send(Uint8Array.from(data, character => character.charCodeAt(0) & 0xff));
	}

	private send(bytes: Uint8Array): void {
		if (this.state === 'ended' || !bytes.byteLength) {
			return;
		}
		if (this.pendingBytes + bytes.byteLength > pendingInputBytes) {
			void this.deliver(formatMessageForTerminal(nls.localize('terminal.inputDropped', 'Some text wasn\'t sent because the shell isn\'t keeping up.')));
			return;
		}
		for (let offset = 0; offset < bytes.byteLength; offset += chunkBytes) {
			this.pending.push(bytes.subarray(offset, offset + chunkBytes));
		}
		this.pendingBytes += bytes.byteLength;
		this.flush();
	}

	public resize(cols: number, rows: number): void {
		cols = clamp(cols, MAX_TERMINAL_COLUMNS);
		rows = clamp(rows, MAX_TERMINAL_ROWS);
		if (this.state === 'ended' || (cols === this.cols && rows === this.rows)) {
			return;
		}
		this.cols = cols;
		this.rows = rows;
		this.channel?.resize(rows, cols);
	}

	public shutdown(): void {
		this.end();
	}

	public override dispose(): void {
		this.end();
		super.dispose();
	}

	private isStarting(): boolean {
		return this.state === 'starting';
	}

	private async refusal(): Promise<TerminalLaunchFailure | undefined> {
		try {
			const report = await this.devices.availability(this.deviceId);
			if (!report) {
				return 'disconnected';
			}
			return launchFailure(report.find(entry => entry.capability === 'terminal')?.status ?? 'unsupported');
		} catch {
			return 'failed';
		}
	}

	private async session(): Promise<{ readonly id: string; readonly generation: string } | undefined> {
		if (!this.engine.available) {
			return undefined;
		}
		try {
			const reply = await this.engine.execute({ type: 'controllerSessions' });
			if (reply.type !== 'sessions') {
				return undefined;
			}
			return reply.sessions.find(item => item.role === 'Controller' && item.peer_id === this.deviceId && item.phase === 'Active');
		} catch {
			return undefined;
		}
	}

	private async consume(packet: DataPacket): Promise<void> {
		if (this.state !== 'running') {
			return;
		}
		switch (packet.kind) {
			case 'terminal':
				if (!packet.data) {
					throw new Error('Terminal output without data');
				}
				return this.deliver(packet.data);
			case 'gap':
				return this.deliver(formatMessageForTerminal(nls.localize('terminal.outputGap', 'Some output was skipped because the connection fell behind.')));
			case 'exited':
				this.finish({ code: exitCode(packet.metadata) });
				return;
			case 'failed':
				this.finish({ reason: 'failed' });
				return;
		}
		throw new Error('Unexpected terminal packet');
	}

	/** Resolves once the emulator acknowledges, or when the shell ends first. */
	private deliver(data: string | Uint8Array): Promise<void> {
		return new Promise<void>(resolve => {
			let done = false;
			const release = (): void => {
				if (!done) {
					done = true;
					if (this.releaseOutput === release) {
						this.releaseOutput = undefined;
					}
					resolve();
				}
			};
			this.releaseOutput = release;
			this.dataEmitter.fire({ data, acknowledge: release });
		});
	}

	private flush(): void {
		const channel = this.channel;
		while (channel && this.state === 'running' && this.inFlight < writesInFlight) {
			const chunk = this.pending.shift();
			if (!chunk) {
				return;
			}
			this.pendingBytes -= chunk.byteLength;
			this.inFlight++;
			channel.write(chunk).then(() => {
				this.inFlight--;
				this.flush();
			}, () => {
				// A failed write fails the channel, which reports the lost connection.
				this.inFlight--;
			});
		}
	}

	private finish(exit: TerminalExit): void {
		if (this.state !== 'running') {
			return;
		}
		this.end();
		this.exitEmitter.fire(exit);
	}

	private end(): void {
		this.state = 'ended';
		const channel = this.channel;
		this.channel = undefined;
		channel?.close();
		this.pending.length = 0;
		this.pendingBytes = 0;
		this.releaseOutput?.();
	}
}

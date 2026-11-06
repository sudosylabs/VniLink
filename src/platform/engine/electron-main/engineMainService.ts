// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { MessageChannelMain, utilityProcess } from 'electron';
import type { MessagePortMain } from 'electron';
import { join } from 'node:path';
import { StringDecoder } from 'node:string_decoder';
import { CancellationTokenSource } from '../../../base/common/cancellation.js';
import { Emitter, Event, Relay } from '../../../base/common/event.js';
import { Disposable, DisposableStore, type IDisposable } from '../../../base/common/lifecycle.js';
import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';
import { Client as MessagePortClient } from '../../../base/parts/ipc/electron-main/ipc.mp.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';
import type { DataChannelRequest } from '../common/dataChannel.js';
import type { EngineCommand, EngineReply } from '../common/engine.js';
import { engineErrorCategory, isEngineEvent, isEngineReply, record } from '../common/engine.js';
import { ENGINE_HOST_CHANNEL_NAME, ENGINE_HOST_CONNECTION, type EngineEventBatch } from '../common/engineService.js';

export const IEngineMainService = createDecorator<EngineMainService>('engineMainService');

/**
 * The engine as main's settings use it: its commands, whether it is running,
 * and the restart state that a relay change can alter.
 */
export interface IEngineSettingsService {
	readonly available: boolean;
	readonly onDidChangeAvailability: Event<boolean>;
	execute(command: EngineCommand): Promise<EngineReply>;
	/** Reads whether a restart is required again and reports a change. */
	refreshRestartRequired(): Promise<void>;
}

/** The IPC connection to the engine host. */
export interface IEngineConnection extends IDisposable {
	readonly channel: IChannel;
}

/**
 * The engine's utility process as the supervisor drives it, after VS Code's
 * `UtilityProcess`: what it prints on standard error, its messages and its
 * exit arrive as events, and `connect` opens the IPC connection to its host.
 */
export interface IEngineProcess {
	readonly onStderr: Event<Uint8Array>;
	readonly onMessage: Event<unknown>;
	readonly onExit: Event<number>;
	connect(): IEngineConnection;
	postMessage(message: unknown, transfer?: MessagePortMain[]): void;
	kill(): void;
}

/** Starts an engine process with `args`. */
type EngineLauncher = (args: readonly string[]) => IEngineProcess;

/** Forks `engine.cjs` as an Electron utility process. */
function forkEngine(args: readonly string[]): IEngineProcess {
	const child = utilityProcess.fork(join(__dirname, 'engine.cjs'), [...args], { serviceName: 'VniLink Engine', stdio: 'pipe' });
	child.stdout?.resume();
	const stderr = child.stderr;
	return {
		onStderr: stderr ? Event.fromNodeEventEmitter<Uint8Array>(stderr, 'data') : () => Disposable.None,
		onMessage: Event.fromNodeEventEmitter<unknown>(child, 'message'),
		onExit: Event.fromNodeEventEmitter<number>(child, 'exit'),
		connect: () => {
			const { port1, port2 } = new MessageChannelMain();
			child.postMessage(ENGINE_HOST_CONNECTION, [port2]);
			const client = new MessagePortClient(port1, 'main');
			return {
				channel: client.getChannel(ENGINE_HOST_CHANNEL_NAME),
				dispose: () => {
					client.dispose();
				},
			};
		},
		postMessage: (message, transfer) => {
			child.postMessage(message, transfer);
		},
		kill: () => {
			child.kill();
		},
	};
}

/** The event stream while no engine is connected. */
const noEvents: Event<EngineEventBatch> = () => Disposable.None;

/** At most this many engine requests may wait at once. */
const maxPendingRequests = 64;

function timeoutFor(command: EngineCommand | { type: 'shutdown' }): number {
	// No call waits for a dial; these two wait for the operating system.
	if (command.type === 'shutdown' || command.type === 'startNativeScreenSharing') {
		return 30000;
	}
	return 10000;
}

/**
 * The engine is restarted this many times in a row, after waits of one second
 * doubling to sixteen, before it is given up for the rest of the run.
 */
const maxRestartAttempts = 5;

const firstRestartDelay = 1000;

const maxRestartDelay = 16000;

/** A restarted engine that stays up this long has earned its attempts back. */
const stableAfter = 60_000;

/**
 * The engine refused this computer's saved installation as unreadable, as
 * one written by an older build is. Starting it again cannot change that, so
 * it is not restarted; its person may start fresh instead.
 */
class UnreadableInstallationError extends Error { }

/**
 * Owns the engine utility process, its request lanes and its lifetime. An
 * engine that exits on its own is started again with a bounded backoff; the
 * restart handler re-applies what main configured on the first start, and
 * availability returns after that.
 */
export class EngineMainService extends Disposable implements IEngineSettingsService {
	declare public readonly _serviceBrand: undefined;
	private readonly availabilityChanged = this._register(new Emitter<boolean>());
	public readonly onDidChangeAvailability = this.availabilityChanged.event;
	private readonly restartRequiredChanged = this._register(new Emitter<boolean>());
	public readonly onDidChangeRestartRequired = this.restartRequiredChanged.event;
	private readonly events = this._register(new Relay<EngineEventBatch>());
	/** Engine events, validated here; the host subscribes only while this has listeners. */
	public readonly onDidReceiveEvents = this.events.event;
	private readonly restartingChanged = this._register(new Emitter<boolean>());
	/** Whether the engine stopped on its own and is waiting to be started again. */
	public readonly onDidChangeRestarting = this.restartingChanged.event;
	private readonly installationUnreadableChanged = this._register(new Emitter<boolean>());
	/** Whether the engine is stopped because it could not read this computer's installation. */
	public readonly onDidChangeInstallationUnreadable = this.installationUnreadableChanged.event;
	private restartRequired = false;
	private restarting = false;
	private installationUnreadable = false;
	private restartAttempts = 0;
	private restartTimer: ReturnType<typeof setTimeout> | undefined;
	private stableTimer: ReturnType<typeof setTimeout> | undefined;
	private restartHandler: (() => Promise<void>) | undefined;
	private launchArguments: readonly string[] = [];
	private ready = false;
	private child: IEngineProcess | undefined;
	/** The listeners on each engine process, released when it exits. */
	private readonly processListeners = this._register(new DisposableStore());
	private connection: IEngineConnection | undefined;
	private channel: IChannel | undefined;
	private connected = false;
	private started = false;
	private closing = false;
	private fastQueue: Promise<void> = Promise.resolve();
	private slowQueue: Promise<void> = Promise.resolve();
	private pending = 0;
	private readonly waiting = new Set<(error: Error) => void>();
	private inflightSnapshot: Promise<EngineReply> | undefined;

	/**
	 * The engine keeps its own log (`vnilink-log`); what it prints on
	 * standard error, such as a native panic, goes to the main log.
	 * `launcher` forks the Electron utility process unless a test gives its own.
	 */
	constructor(
		@ILogService private readonly logService: ILogService,
		private readonly launcher: EngineLauncher = forkEngine,
	) {
		super();
	}

	public get available(): boolean {
		return this.ready && !this.closing;
	}

	public get isRestarting(): boolean {
		return this.restarting;
	}

	/** The engine is stopped, not restarting, because this computer's installation could not be read. */
	public get isInstallationUnreadable(): boolean {
		return this.installationUnreadable;
	}

	/**
	 * Registers what runs on every engine started after the first, before
	 * windows are told it is available again: the engine begins empty, so the
	 * defaults, settings and permissions main applied at launch are applied
	 * again. `available` already reads true while it runs.
	 */
	public setRestartHandler(handler: () => Promise<void>): void {
		this.restartHandler = handler;
	}

	/**
	 * `capture` lets a temporary engine host this computer's screen; only the
	 * latency measurement asks for it. `screenGrant` is the file such an engine
	 * keeps the desktop portal's grant in, so a measurement asks once.
	 */
	public async start(temporary = false, capture = false, screenGrant?: string): Promise<void> {
		if (this.started || this.closing) {
			throw new Error('Engine already started');
		}
		this.started = true;
		this.launchArguments = temporary ? ['--temporary', ...(capture ? ['--capture', ...(screenGrant === undefined ? [] : ['--screen-grant', screenGrant])] : [])] : [];
		try {
			await this.launch();
		} catch (error) {
			// A first start that fails is retried like a crash, unless retrying cannot help; the restart handler
			// configures the engine that comes up.
			this.afterFailedStart(error);
			throw error;
		}
		this.markAvailable();
	}

	/**
	 * Sets aside the installation the engine could not read and starts it on
	 * a new one, as its person chose: this computer becomes new to its paired
	 * computers. Refused unless that is why the engine is stopped; the engine
	 * itself also keeps an installation it can read.
	 *
	 * The notice stays until this start is ready. A failure leaves it,
	 * including one that set the database aside and then could not finish,
	 * so the same action can be tried again. A plain restart would open
	 * those same files.
	 */
	public async startFresh(): Promise<void> {
		if (!this.installationUnreadable || this.closing || this.child) {
			throw new Error('The engine has no unreadable installation to start fresh from');
		}
		this.setRestarting(true);
		try {
			await this.launch(['--start-fresh']);
			this.ready = true;
			await this.restartHandler?.();
			this.setInstallationUnreadable(false);
			this.markAvailable();
		} catch (error) {
			this.logService.error('[engine] could not start fresh.', error);
			this.fail(error instanceof Error ? error : new Error('Engine start failed'));
			this.setRestarting(false);
			this.setInstallationUnreadable(true);
			throw error;
		}
	}

	/** Forks the engine, with `extra` arguments this once, and resolves once it accepts commands; it stays supervised from then on. */
	private async launch(extra: readonly string[] = []): Promise<void> {
		const child = this.launcher([...this.launchArguments, ...extra]);
		const listeners = this.processListeners.add(new DisposableStore());
		// A line may arrive in several chunks; each is logged once it is whole, the last when the engine exits.
		let partial = '';
		const logLine = (line: string): void => {
			if (line.trim()) {
				this.logService.error(`[engine] ${line.slice(0, 2000)}`);
			}
		};
		// The decoder keeps a character split across chunks whole.
		const decoder = new StringDecoder('utf8');
		listeners.add(child.onStderr(chunk => {
			const lines = (partial + decoder.write(chunk)).split('\n');
			partial = lines.pop() ?? '';
			lines.forEach(logLine);
		}));
		listeners.add(Event.once(child.onExit)(() => {
			logLine(partial + decoder.end());
			partial = '';
		}));
		this.child = child;
		let supervised = false;
		let abandon: ((error: Error) => void) | undefined;
		const started = new Promise<void>((resolve, reject) => {
			const timeout = setTimeout(() => {
				this.fail(new Error('Engine startup timed out'));
				reject(new Error('Engine startup timed out'));
			}, 60_000);
			// `stop` fails everything waiting, this start included, at once.
			abandon = (error: Error): void => {
				clearTimeout(timeout);
				reject(error);
			};
			this.waiting.add(abandon);
			listeners.add(child.onMessage(message => {
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
						supervised = true;
						resolve();
					} else {
						const reason = typeof message.error === 'string' ? `: ${message.error}` : '';
						const unreadable = typeof message.error === 'string' && engineErrorCategory(new Error(message.error)) === 'CorruptRecord';
						const error = new (unreadable ? UnreadableInstallationError : Error)(`Engine could not open its secure installation${reason}`);
						this.fail(error);
						reject(error);
					}
				}
			}));
			listeners.add(Event.once(child.onExit)(code => {
				clearTimeout(timeout);
				if (this.child !== child) {
					// Stopped by `fail`, which already reported why.
					return;
				}
				const error = new Error(`Engine exited (${String(code)})`);
				if (supervised) {
					// Before availability drops, so windows never see an engine that is neither up nor coming back.
					this.scheduleRestart();
				}
				this.fail(error);
				reject(error);
			}));
		});
		// Nothing more is heard from an engine that exited.
		listeners.add(Event.once(child.onExit)(() => {
			this.processListeners.delete(listeners);
		}));
		try {
			await started;
		} finally {
			if (abandon) {
				this.waiting.delete(abandon);
			}
		}
	}

	private markAvailable(): void {
		if (this.closing || !this.connected) {
			return;
		}
		this.ready = true;
		this.setRestarting(false);
		this.availabilityChanged.fire(true);
		clearTimeout(this.stableTimer);
		this.stableTimer = setTimeout(() => {
			this.restartAttempts = 0;
		}, stableAfter);
	}

	private setRestarting(value: boolean): void {
		if (value !== this.restarting) {
			this.restarting = value;
			this.restartingChanged.fire(value);
		}
	}

	private setInstallationUnreadable(value: boolean): void {
		if (value !== this.installationUnreadable) {
			this.installationUnreadable = value;
			this.installationUnreadableChanged.fire(value);
		}
	}

	/** Restarts after a start that failed, unless the installation could not be read, which restarting cannot fix. */
	private afterFailedStart(error: unknown): void {
		if (error instanceof UnreadableInstallationError) {
			this.logService.error('[engine] this computer\'s installation could not be read; it is not started again until its person starts fresh.');
			this.setRestarting(false);
			this.setInstallationUnreadable(true);
			return;
		}
		this.scheduleRestart();
	}

	/** Starts the engine again after a wait that doubles with each failure, until the attempts are spent. */
	private scheduleRestart(): void {
		if (this.closing || this.restartTimer) {
			return;
		}
		if (this.restartAttempts >= maxRestartAttempts) {
			this.logService.error(`[engine] stopped ${String(maxRestartAttempts)} times in a row; it will not be started again until VniLink restarts.`);
			this.setRestarting(false);
			return;
		}
		const delay = Math.min(firstRestartDelay * 2 ** this.restartAttempts, maxRestartDelay);
		this.restartAttempts++;
		this.setRestarting(true);
		this.restartTimer = setTimeout(() => {
			this.restartTimer = undefined;
			void this.restart();
		}, delay);
	}

	private async restart(): Promise<void> {
		if (this.closing) {
			return;
		}
		try {
			await this.launch();
			// Main's services see the new engine as available while they configure it; windows hear of it after.
			this.ready = true;
			await this.restartHandler?.();
			this.markAvailable();
		} catch (error) {
			this.logService.error('[engine] could not be started again.', error);
			this.fail(error instanceof Error ? error : new Error('Engine restart failed'));
			this.afterFailedStart(error);
		}
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
	private connect(child: IEngineProcess): void {
		this.connection = child.connect();
		this.channel = this.connection.channel;
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
				// A slow call fails alone; the engine stays up for the rest. Cancelling
				// stops the wait, not the work: the command may still finish, so the
				// message says so and callers re-read before they repeat a change.
				this.logService.warn(`[engine] ${command.type} did not answer in time; its outcome is unknown.`);
				cancellation.cancel();
				fail(new Error('Engine operation timed out and may still finish'));
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
		clearTimeout(this.restartTimer);
		this.restartTimer = undefined;
		this.setRestarting(false);
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
		this.ready = false;
		this.connected = false;
		clearTimeout(this.stableTimer);
		for (const reject of [...this.waiting]) {
			reject(error);
		}
		this.events.input = noEvents;
		this.channel = undefined;
		this.connection?.dispose();
		this.connection = undefined;
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

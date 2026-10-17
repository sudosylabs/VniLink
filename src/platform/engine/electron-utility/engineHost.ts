import { createRequire } from 'node:module';
import { join } from 'node:path';
import { Emitter } from '../../../base/common/event.js';
import type { IClientConnectionFilter } from '../../../base/parts/ipc/node/ipc.mp.js';
import type { MessageEvent } from '../../../base/parts/sandbox/node/electronTypes.js';
import { isChannelRequest } from '../common/dataChannel.js';
import { record } from '../common/engine.js';
import type { EngineEvent } from '../common/engine.js';
import type { EngineEventBatch } from '../common/engineService.js';
import { DataChannelHost } from './dataChannelHost.js';
import type { NativeDataChannel } from './dataChannelHost.js';

interface NativeEngine { execute(command: string): Promise<string>; openDataChannel(kind: string, sessionId: string, generation: string, rows: number, columns: number, media?: string): Promise<NativeDataChannel>; }

interface Addon { openEngine(): Promise<NativeEngine>; openTemporaryEngine(): Promise<NativeEngine>; }

function isAddon(value: unknown): value is Addon {
	return record(value) && typeof value.openEngine === 'function' && typeof value.openTemporaryEngine === 'function';
}

/** Engine events are read this often while the main process listens. */
const eventPollInterval = 500;

/** Each window may own at most this many native data channels. */
const maxChannelsPerOwner = 8;

/**
 * Hosts the native engine in the utility process. Main reaches it through
 * the `engineHost` IPC channel; data-channel ports arrive as separate
 * parent-port messages, which this host handles before the IPC server.
 */
export class EngineHost implements IClientConnectionFilter {
	private engine: NativeEngine | undefined;
	private shuttingDown = false;
	private readonly inputSessions = new Set<string>();
	private readonly channels = new Map<string, Set<DataChannelHost>>();
	private readonly eventsEmitter = new Emitter<EngineEventBatch>({
		onWillAddFirstListener: () => {
			this.startEvents();
		},
		onDidRemoveLastListener: () => {
			this.stopEvents();
		},
	});
	public readonly onDidReceiveEvents = this.eventsEmitter.event;
	private subscription: string | undefined;
	private eventTimer: ReturnType<typeof setTimeout> | undefined;
	private eventsWanted = false;
	private resyncPending = true;

	constructor(
		private readonly port: Pick<Electron.ParentPort, 'postMessage'>,
		private readonly loadAddon: () => unknown = () => createRequire(__filename)(join(__dirname, 'native', 'vnilink.node')),
	) { }

	public async open(): Promise<void> {
		try {
			const addon = this.loadAddon();
			if (!isAddon(addon)) {
				throw new Error('Invalid addon');
			}
			this.engine = await (process.argv.includes('--temporary') ? addon.openTemporaryEngine() : addon.openEngine());
			this.port.postMessage({ ready: true });
		} catch {
			this.port.postMessage({ ready: false });
			process.exit(1);
		}
	}

	/**
	 * Runs one engine command. Commands are shape-checked only: validation
	 * lives at the window IPC boundary and in Rust, so a bad caller fails its
	 * own request without taking the engine down.
	 */
	public async execute(command: unknown): Promise<unknown> {
		if (!record(command)) {
			throw new Error('Invalid engine command');
		}
		if (!this.engine) {
			throw new Error('Engine unavailable');
		}
		if (this.shuttingDown) {
			throw new Error('Engine is shutting down');
		}
		if (command.type === 'shutdown' && Object.keys(command).length === 1) {
			this.beginShutdown();
		}
		try {
			// Replies are validated by the main-process owner; forward them untouched.
			return JSON.parse(await this.engine.execute(JSON.stringify(command)));
		} catch (error) {
			// Native error strings name the reason only, never secrets.
			const reason = error instanceof Error && error.message.length > 0 ? error.message : 'Engine operation failed';
			throw new Error(reason.slice(0, 512));
		}
	}

	public handledClientConnection(event: MessageEvent): boolean {
		const data: unknown = event.data;
		if (record(data) && data.type === 'closeDataChannels') {
			if (typeof data.owner === 'string') {
				this.closeChannels(data.owner);
			}
			return true;
		}
		if (record(data) && data.type === 'attachDataChannel') {
			this.attachChannel(data.request, data.owner, event.ports);
			return true;
		}
		return false;
	}

	private attachChannel(request: unknown, owner: unknown, ports: readonly Electron.MessagePortMain[]): void {
		const port = ports.at(0);
		if (!port || !this.engine || this.shuttingDown || typeof owner !== 'string' || !isChannelRequest(request)) {
			for (const candidate of ports) {
				candidate.close();
			}
			return;
		}
		const channels = this.channels.get(owner) ?? new Set<DataChannelHost>();
		const inputKey = `${request.sessionId}:${request.generation}`;
		if (channels.size >= maxChannelsPerOwner || (request.kind === 'input' && this.inputSessions.has(inputKey))) {
			port.postMessage({ type: 'error' });
			port.close();
			return;
		}
		if (request.kind === 'input') {
			this.inputSessions.add(inputKey);
		}
		this.channels.set(owner, channels);
		const channel = new DataChannelHost(port, request, () => {
			channels.delete(channel);
			if (request.kind === 'input') {
				this.inputSessions.delete(inputKey);
			}
			if (channels.size === 0) {
				this.channels.delete(owner);
			}
		});
		channels.add(channel);
		void this.engine.openDataChannel(request.kind, request.sessionId, request.generation, request.rows ?? 24, request.columns ?? 80, request.media ? JSON.stringify(request.media) : undefined).then(native => {
			channel.start(native);
		}, () => {
			channel.fail();
		});
	}

	private closeChannels(owner: string): void {
		for (const channel of this.channels.get(owner) ?? []) {
			channel.close();
		}
		this.channels.delete(owner);
	}

	private beginShutdown(): void {
		this.shuttingDown = true;
		this.stopEvents();
		for (const owner of [...this.channels.keys()]) {
			this.closeChannels(owner);
		}
		this.inputSessions.clear();
	}

	private startEvents(): void {
		this.eventsWanted = true;
		this.resyncPending = true;
		this.scheduleEvents(0);
	}

	private stopEvents(): void {
		this.eventsWanted = false;
		clearTimeout(this.eventTimer);
		this.eventTimer = undefined;
		this.releaseSubscription();
	}

	private scheduleEvents(delay: number): void {
		clearTimeout(this.eventTimer);
		this.eventTimer = setTimeout(() => {
			void this.readEvents();
		}, delay);
	}

	/** Reads one batch of events, subscribing first when needed, and reports gaps as a resync. */
	private async readEvents(): Promise<void> {
		if (!this.readingEvents()) {
			return;
		}
		try {
			if (!this.subscription) {
				const reply = await this.execute({ type: 'subscribeEvents' });
				if (!record(reply) || reply.type !== 'eventSubscription' || typeof reply.subscriptionId !== 'string') {
					throw new Error('Invalid event subscription');
				}
				this.subscription = reply.subscriptionId;
				if (!this.readingEvents()) {
					this.releaseSubscription();
					return;
				}
			}
			const reply = await this.execute({ type: 'pollEvents', subscriptionId: this.subscription, maxEvents: 256 });
			if (!record(reply) || reply.type !== 'events' || !Array.isArray(reply.events)) {
				throw new Error('Invalid event reply');
			}
			const resync = this.resyncPending || reply.lagged !== '0';
			this.resyncPending = false;
			if (resync || reply.events.length) {
				this.eventsEmitter.fire({ events: reply.events as EngineEvent[], resync });
			}
		} catch {
			// The next successful read reports that events may have been missed.
			this.releaseSubscription();
			this.resyncPending = true;
		} finally {
			if (this.readingEvents()) {
				this.scheduleEvents(eventPollInterval);
			}
		}
	}

	/** Re-read after every await: listeners and shutdown change while a read is in flight. */
	private readingEvents(): boolean {
		return this.eventsWanted && !this.shuttingDown;
	}

	private releaseSubscription(): void {
		const subscriptionId = this.subscription;
		this.subscription = undefined;
		if (subscriptionId && this.engine && !this.shuttingDown) {
			void this.execute({ type: 'releaseEvents', subscriptionId }).catch(() => undefined);
		}
	}
}

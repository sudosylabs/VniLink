import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { EngineEvent, SessionInfo } from '../../engine/common/engine.js';
import type { EngineEventBatch } from '../../engine/common/engineService.js';
import { IEngineMainService } from '../../engine/electron-main/engineMainService.js';
import type { EngineMainService } from '../../engine/electron-main/engineMainService.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';

export const IHostingMainService = createDecorator<HostingMainService>('hostingMainService');

/** Whether this computer accepts paired devices, and who is connected to it. */
export interface HostingStatus {
	readonly hosting: boolean;
	readonly incoming: SessionInfo | null;
}

const statusEvents = new Set<EngineEvent['type']>(['hostingChanged', 'sessionChanged']);

const stopped: HostingStatus = { hosting: false, incoming: null };

/**
 * Keeps this computer reachable for as long as the application runs, with or
 * without a window, and follows the engine's events so main-process surfaces
 * can show what is happening. Rust still gates every service with global
 * consent and unknown peers with owner confirmation.
 */
export class HostingMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly statusChanged = this._register(new Emitter<HostingStatus>());
	public readonly onDidChangeStatus = this.statusChanged.event;
	private current = stopped;
	private started = false;
	/** Set when the last `startHosting` failed, so the next resync tries again. */
	private hostingFailed = false;
	private refreshing: Promise<void> | undefined;
	private refreshAgain = false;

	constructor(
		@IEngineMainService private readonly engine: EngineMainService,
		@ILogService private readonly logService: ILogService,
	) {
		super();
	}

	public get status(): HostingStatus {
		return this.current;
	}

	/** Starts hosting and listens to engine events until the application quits. */
	public start(): void {
		if (this.started) {
			return;
		}
		this.started = true;
		// Listening keeps the engine host polling while no window is open.
		this._register(this.engine.onDidReceiveEvents(batch => {
			this.onEvents(batch);
		}));
		this._register(this.engine.onDidChangeAvailability(available => {
			if (available) {
				void this.startHosting();
			} else {
				this.update(stopped);
			}
		}));
		if (this.engine.available) {
			void this.startHosting();
		}
	}

	private async startHosting(): Promise<void> {
		try {
			await this.engine.execute({ type: 'startHosting' });
			this.hostingFailed = false;
		} catch (error) {
			this.hostingFailed = true;
			this.logService.error('This computer could not start accepting paired devices.', error);
		}
		await this.refresh();
	}

	private onEvents(batch: EngineEventBatch): void {
		if (batch.resync && this.hostingFailed) {
			void this.startHosting();
		} else if (batch.resync || batch.events.some(event => statusEvents.has(event.type))) {
			void this.refresh();
		}
	}

	/** Reads the snapshot; events during a read schedule one more read rather than overlapping. */
	private refresh(): Promise<void> {
		if (this.refreshing) {
			this.refreshAgain = true;
			return this.refreshing;
		}
		this.refreshing = (async () => {
			do {
				this.refreshAgain = false;
				try {
					const reply = await this.engine.execute({ type: 'snapshot' });
					if (reply.type === 'snapshot') {
						this.update({ hosting: reply.snapshot.hosting, incoming: reply.snapshot.incoming });
					}
				} catch {
					// Unavailability is reported through `onDidChangeAvailability`.
				}
			} while (this.shouldRefreshAgain());
		})().finally(() => {
			this.refreshing = undefined;
		});
		return this.refreshing;
	}

	/** Read separately: `refreshAgain` changes while a read is awaited. */
	private shouldRefreshAgain(): boolean {
		return this.refreshAgain && !this._store.isDisposed;
	}

	private update(status: HostingStatus): void {
		if (this._store.isDisposed || JSON.stringify(status) === JSON.stringify(this.current)) {
			return;
		}
		this.current = status;
		this.statusChanged.fire(status);
	}
}

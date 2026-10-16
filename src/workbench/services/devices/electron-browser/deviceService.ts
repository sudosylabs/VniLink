import { Emitter } from '../../../../base/common/event.js';
import { RunOnceScheduler } from '../../../../base/common/async.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import type { DesktopApi } from '../../../../platform/ipc/common/desktopApi.js';
import type { EngineReply } from '../../../../platform/engine/common/engine.js';
import type { DeviceSummary, IDeviceService } from '../common/deviceService.js';

export class DeviceService extends Disposable implements IDeviceService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<readonly DeviceSummary[]>());
	public readonly onDidChangeDevices = this.changed.event;
	private current: readonly DeviceSummary[] = Object.freeze([]);
	private revision = 0;
	private subscription: string | undefined;
	private needsRefresh = true;
	private readonly lifetime = new AbortController();
	private readonly polling = this._register(new RunOnceScheduler(() => {
		void this.poll();
	}, 500));

	constructor(
		private readonly api: Pick<DesktopApi, 'engineCommand'>,
		public readonly available: boolean,
	) {
		super();
	}

	private started = false;

	public async initialize(): Promise<void> {
		if (this.started || !this.available || this.isDisposed()) {
			return;
		}
		this.started = true;
		await this.start();
	}

	public get devices(): readonly DeviceSummary[] {
		return this.current;
	}

	public async refresh(signal: AbortSignal): Promise<void> {
		if (signal.aborted || this.isDisposed()) {
			return;
		}
		if (!this.available) {
			throw new Error('Device service unavailable');
		}
		const revision = ++this.revision;
		let reply: EngineReply;
		try {
			reply = await this.api.engineCommand({ type: 'snapshot' });
		} catch (error) {
			if (!this.isCurrentRequest(revision, signal)) {
				return;
			}
			throw error;
		}
		// IPC cannot cancel this request; fence its result instead.
		if (!this.isCurrentRequest(revision, signal)) {
			return;
		}
		if (reply.type !== 'snapshot') {
			throw new Error('Expected engine snapshot');
		}
		const next = Object.freeze(reply.snapshot.devices.map(device => Object.freeze({
			id: device.endpoint_id,
			name: device.pet_name ?? device.hostname ?? device.endpoint_id,
			os: device.os,
			presence: device.presence,
		})));
		if (JSON.stringify(next) !== JSON.stringify(this.current)) {
			this.current = next;
			this.changed.fire(this.current);
		}
	}

	private async start(): Promise<void> {
		try {
			const reply = await this.api.engineCommand({ type: 'subscribeEvents' });
			if (reply.type !== 'eventSubscription') {
				throw new Error('Invalid device event subscription');
			}
			if (this.isDisposed()) {
				await this.api.engineCommand({ type: 'releaseEvents', subscriptionId: reply.subscriptionId });
				return;
			}
			this.subscription = reply.subscriptionId;
			await this.refresh(this.lifetime.signal);
			this.needsRefresh = false;
		} catch {
			this.markUnknown();
		} finally {
			if (!this.isDisposed()) {
				this.polling.schedule();
			}
		}
	}

	private async poll(): Promise<void> {
		if (this.isDisposed()) {
			return;
		}
		if (!this.subscription) {
			await this.start();
			return;
		}
		try {
			const reply = await this.api.engineCommand({ type: 'pollEvents', subscriptionId: this.subscription, maxEvents: 256 });
			if (reply.type !== 'events') {
				throw new Error('Invalid device event reply');
			}
			if (this.needsRefresh || reply.lagged !== '0' || reply.events.some(event => ['devicePaired', 'deviceAuthorized', 'deviceRevoked', 'deviceUnpaired', 'peerRenamed', 'sessionChanged'].includes(event.type))) {
				await this.refresh(this.lifetime.signal);
				this.needsRefresh = false;
			}
		} catch {
			this.needsRefresh = true;
			const subscriptionId = this.subscription;
			this.subscription = undefined;
			if (subscriptionId) {
				await this.api.engineCommand({ type: 'releaseEvents', subscriptionId }).catch(() => undefined);
			}
			this.markUnknown();
		} finally {
			if (!this.isDisposed()) {
				this.polling.schedule();
			}
		}
	}

	private markUnknown(): void {
		if (this.isDisposed() || this.current.every(device => device.presence === 'unknown')) {
			return;
		}
		this.current = Object.freeze(this.current.map(device => Object.freeze({ ...device, presence: 'unknown' as const })));
		this.changed.fire(this.current);
	}

	public override dispose(): void {
		this.lifetime.abort();
		const subscriptionId = this.subscription;
		this.subscription = undefined;
		super.dispose();
		if (subscriptionId) {
			void this.api.engineCommand({ type: 'releaseEvents', subscriptionId }).catch(() => undefined);
		}
	}

	private isCurrentRequest(revision: number, signal: AbortSignal): boolean {
		return !signal.aborted && !this.isDisposed() && revision === this.revision;
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}

}

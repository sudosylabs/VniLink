import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { IEngineService, type EngineEventBatch } from '../../../platform/engine/common/engineService.js';
import { deviceName, type EngineCommand, type EngineEvent, type EngineReply, type ControlRequestInfo, type RemoteCapabilityStatus } from '../../../platform/engine/common/engine.js';

/** Engine events after which the device list may have changed. */
const deviceEvents: ReadonlySet<EngineEvent['type']> = new Set(['devicePaired', 'deviceAuthorized', 'deviceRevoked', 'deviceUnpaired', 'peerRenamed', 'sessionChanged', 'presenceChanged']);

import { type DeviceSummary, IDeviceService, type PairingFile } from './deviceService.js';

export class DeviceService extends Disposable implements IDeviceService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<readonly DeviceSummary[]>());
	public readonly onDidChangeDevices = this.changed.event;
	private readonly availabilityChanged = this._register(new Emitter<string>());
	public readonly onDidChangeAvailability = this.availabilityChanged.event;
	private readonly stateChanged = this._register(new Emitter<void>());
	public readonly onDidChangeState = this.stateChanged.event;
	private loaded = false;
	private failed = false;
	private current: readonly DeviceSummary[] = Object.freeze([]);
	private revision = 0;
	private readonly lifetime = new AbortController();

	constructor(@IEngineService private readonly engine: IEngineService) {
		super();
	}

	public get available(): boolean {
		return this.engine.available;
	}

	private started = false;

	public async initialize(): Promise<void> {
		if (this.started || !this.available || this.isDisposed()) {
			return;
		}
		this.started = true;
		this._register(this.engine.onDidReceiveEvents(batch => {
			this.onEvents(batch);
		}));
		this._register(this.engine.onDidChangeAvailability(available => {
			if (available) {
				void this.start();
			} else {
				this.markUnknown();
			}
		}));
		await this.start();
	}

	public get state(): 'loading' | 'ready' | 'unavailable' {
		return !this.available || this.failed ? 'unavailable' : this.loaded ? 'ready' : 'loading';
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
			reply = await this.engine.execute({ type: 'snapshot' });
		} catch (error) {
			if (!this.isCurrentRequest(revision, signal)) {
				return;
			}
			this.setState(true);
			throw error;
		}
		// IPC cannot cancel this request; fence its result instead.
		if (!this.isCurrentRequest(revision, signal)) {
			return;
		}
		if (reply.type !== 'snapshot') {
			this.setState(true);
			throw new Error('Expected engine snapshot');
		}
		this.setState(false);
		const next = Object.freeze(reply.snapshot.devices.map(device => Object.freeze({
			id: device.endpoint_id,
			name: deviceName(device),
			os: device.os,
			presence: device.presence,
			hostname: device.hostname,
			direct: device.direct.length,
			relays: device.relays.length,
		})));
		if (JSON.stringify(next) !== JSON.stringify(this.current)) {
			this.current = next;
			this.changed.fire(this.current);
		}
	}

	/** Establish identity/presence without opening any remote service. */
	public async connect(id: string): Promise<void> {
		const reply = await this.execute({ type: 'beginConnectServices', endpointId: id });
		if (reply.type !== 'session') {
			throw new Error('Invalid connection reply');
		}
		await this.refresh(this.lifetime.signal);
	}

	public async rename(id: string, name: string): Promise<void> {
		await this.execute({ type: 'setPeerName', endpointId: id, name });
		await this.refresh(this.lifetime.signal);
	}

	public async forget(id: string): Promise<void> {
		await this.execute({ type: 'unpair', endpointId: id });
		await this.refresh(this.lifetime.signal);
	}

	public async exportPairingFile(): Promise<PairingFile> {
		const reply = await this.execute({ type: 'exportPairingFile', ttlSeconds: 900, allowControl: true });
		if (reply.type !== 'pairingFile') {
			throw new Error('Invalid pairing file reply');
		}
		return { bytes: reply.file.bytes, check: reply.file.check, expiresAt: Number(reply.file.expires_at_ms) };
	}

	public async pair(bytes: Uint8Array, digits: string): Promise<string> {
		const preview = await this.execute({ type: 'openPairingFile', bytes: Array.from(bytes) });
		if (preview.type !== 'pairingPreview') {
			throw new Error('Invalid pairing preview');
		}
		// Consume every opened handle, including a failed code check. Do not keep
		// engine previews alive while a file picker or confirmation view is open.
		const reply = await this.engine.execute({ type: 'confirmPairing', previewId: preview.preview.id, digits });
		if (reply.type !== 'device') {
			throw new Error('Invalid pairing confirmation');
		}
		await this.refresh(this.lifetime.signal);
		return reply.device.endpoint_id;
	}

	public async pairingCheck(): Promise<string> {
		const reply = await this.execute({ type: 'controlCheck' });
		if (reply.type !== 'check') {
			throw new Error('Invalid pairing check');
		}
		return reply.check;
	}

	public async incomingPairing(): Promise<ControlRequestInfo | null> {
		const reply = await this.execute({ type: 'controlRequest' });
		if (reply.type !== 'controlRequest') {
			throw new Error('Invalid incoming pairing request');
		}
		return reply.request;
	}

	public async approvePairing(digits: string): Promise<void> {
		await this.execute({ type: 'confirmControlRequest', digits });
		await this.refresh(this.lifetime.signal);
	}

	public async rejectPairing(): Promise<void> {
		await this.execute({ type: 'rejectControlRequest' });
	}

	private execute(command: EngineCommand): Promise<EngineReply> {
		if (!this.available || this.isDisposed()) {
			return Promise.reject(new Error('Device service unavailable'));
		}
		return this.engine.execute(command);
	}

	private async start(): Promise<void> {
		try {
			// Hosting is owned by the main process (`HostingMainService`).
			await this.refresh(this.lifetime.signal);
		} catch {
			this.markUnknown();
		}
	}

	public async availability(id: string): Promise<readonly RemoteCapabilityStatus[] | null> {
		const reply = await this.execute({ type: 'peerAvailability', endpointId: id });
		if (reply.type !== 'peerAvailability') {
			throw new Error('Invalid availability reply');
		}
		return reply.report === null ? null : Object.freeze(reply.report.map(entry => Object.freeze({ ...entry })));
	}

	private onEvents(batch: EngineEventBatch): void {
		if (this.isDisposed()) {
			return;
		}
		// A report changes with the report itself and whenever reachability does.
		for (const event of batch.events) {
			if (event.type === 'peerAvailabilityChanged' || event.type === 'presenceChanged') {
				this.availabilityChanged.fire(event.endpoint_id);
			}
		}
		if (!(batch.resync || batch.events.some(event => deviceEvents.has(event.type)))) {
			return;
		}
		void this.refresh(this.lifetime.signal).catch(() => {
			this.markUnknown();
		});
	}

	private setState(failed: boolean): void {
		if (this.isDisposed()) {
			return;
		}
		const previous = this.state;
		this.loaded = true;
		this.failed = failed;
		if (previous !== this.state) {
			this.stateChanged.fire();
		}
	}

	private markUnknown(): void {
		this.setState(true);
		if (this.isDisposed() || this.current.every(device => device.presence === 'unknown')) {
			return;
		}
		this.current = Object.freeze(this.current.map(device => Object.freeze({ ...device, presence: 'unknown' as const })));
		this.changed.fire(this.current);
	}

	public override dispose(): void {
		this.lifetime.abort();
		super.dispose();
	}

	private isCurrentRequest(revision: number, signal: AbortSignal): boolean {
		return !signal.aborted && !this.isDisposed() && revision === this.revision;
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}

}

registerSingleton(IDeviceService, DeviceService, InstantiationType.Delayed);

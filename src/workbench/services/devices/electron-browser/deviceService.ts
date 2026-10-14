import { Emitter } from '../../../../base/common/event.js';
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

	constructor(
		private readonly api: Pick<DesktopApi, 'engineCommand'>,
		public readonly available: boolean,
	) {
		super();
	}

	public get devices(): readonly DeviceSummary[] {
		return this.current;
	}

	public async refresh(signal: AbortSignal): Promise<void> {
		if (signal.aborted || this._store.isDisposed) {
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
		this.current = Object.freeze(reply.snapshot.devices.map(device => Object.freeze({
			id: device.endpoint_id,
			name: device.pet_name ?? device.hostname ?? device.endpoint_id,
		})));
		this.changed.fire(this.current);
	}

	private isCurrentRequest(revision: number, signal: AbortSignal): boolean {
		return !signal.aborted && !this._store.isDisposed && revision === this.revision;
	}
}

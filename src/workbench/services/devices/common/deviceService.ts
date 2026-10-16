import type { OperatingSystem, DevicePresence } from '../../../../platform/engine/common/engine.js';
import type { Event } from '../../../../base/common/event.js';
import { createDecorator } from '../../../../platform/instantiation/common/instantiation.js';

export interface DeviceSummary {
	readonly id: string;
	readonly name: string;
	readonly os: OperatingSystem | null;
	readonly presence: DevicePresence | 'unknown';
}

export const IDeviceService = createDecorator<IDeviceService>('deviceService');

/** Window-owned device state. Views never depend on the preload transport. */
export interface IDeviceService {
	readonly _serviceBrand: undefined;
	readonly available: boolean;
	readonly devices: readonly DeviceSummary[];
	readonly onDidChangeDevices: Event<readonly DeviceSummary[]>;
	initialize(): Promise<void>;
	refresh(signal: AbortSignal): Promise<void>;
}

import type { OperatingSystem, DevicePresence, ControlRequestInfo } from '../../../../platform/engine/common/engine.js';
import type { Event } from '../../../../base/common/event.js';
import { createDecorator } from '../../../../platform/instantiation/common/instantiation.js';

export interface DeviceSummary {
	readonly id: string;
	readonly name: string;
	readonly os: OperatingSystem | null;
	readonly presence: DevicePresence | 'unknown';
}

export interface PairingFile {
	readonly bytes: readonly number[];
	readonly check: string;
	readonly expiresAt: number;
}

export const IDeviceService = createDecorator<IDeviceService>('deviceService');

/** Window-owned device state. Views never depend on the preload transport. */
export interface IDeviceService {
	readonly _serviceBrand: undefined;
	readonly available: boolean;
	readonly state: 'loading' | 'ready' | 'unavailable';
	readonly onDidChangeState: Event<void>;
	readonly devices: readonly DeviceSummary[];
	readonly onDidChangeDevices: Event<readonly DeviceSummary[]>;
	initialize(): Promise<void>;
	refresh(signal: AbortSignal): Promise<void>;
	connect(id: string): Promise<void>;
	rename(id: string, name: string): Promise<void>;
	forget(id: string): Promise<void>;
	exportPairingFile(): Promise<PairingFile>;
	pair(bytes: Uint8Array, digits: string): Promise<string>;
	pairingCheck(): Promise<string>;
	incomingPairing(): Promise<ControlRequestInfo | null>;
	approvePairing(digits: string): Promise<void>;
	rejectPairing(): Promise<void>;
}

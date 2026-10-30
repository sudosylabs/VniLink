// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { OperatingSystem, DevicePresence, ControlRequestInfo, RemoteCapabilityStatus } from '../../../platform/engine/common/engine.js';
import type { Event } from '../../../base/common/event.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

export interface DeviceSummary {
	readonly id: string;
	readonly name: string;
	readonly os: OperatingSystem | null;
	readonly presence: DevicePresence | 'unknown';
	/** The name the computer reports for itself, which a pet name may hide. */
	readonly hostname: string | null;
	/** Known direct addresses. */
	readonly direct: number;
	/** Known relays. */
	readonly relays: number;
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
	/** Fires with a device id when what that computer allows may have changed. */
	readonly onDidChangeAvailability: Event<string>;
	/** What a paired computer last reported it allows; `null` while it is unreachable or before its first report. */
	availability(id: string): Promise<readonly RemoteCapabilityStatus[] | null>;
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

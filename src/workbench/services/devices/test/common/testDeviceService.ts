// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter } from '../../../../../base/common/event.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import type { ControlRequestInfo, RemoteCapabilityStatus } from '../../../../../platform/engine/common/engine.js';
import type { ControlCheck, DeviceSummary, IDeviceService, PairingFile } from '../../deviceService.js';

/** What a pairing flow asked of the device service, in order. */
export type TestDeviceCall =
	| { readonly type: 'approve'; readonly digits: string }
	| { readonly type: 'reject' }
	| { readonly type: 'pair'; readonly bytes: readonly number[]; readonly digits: string }
	| { readonly type: 'connect'; readonly id: string };

/**
 * An {@link IDeviceService} for pairing flows: `incoming` is the pending
 * pairing request, `exportPairingFile` and `pair` can be replaced to script
 * replies, and `fireDevices` and `fireControlCheck` deliver what the engine
 * would report. Approving or rejecting clears `incoming`, as the engine
 * does. Every pairing call is kept in `calls`; device reads and changes it
 * does not script are refused.
 */
export class TestDeviceService extends Disposable implements IDeviceService {
	declare public readonly _serviceBrand: undefined;
	public readonly available = true;
	public readonly state = 'ready';
	public readonly onDidChangeState = this._register(new Emitter<void>()).event;
	public devices: readonly DeviceSummary[] = [];
	private readonly devicesChanged = this._register(new Emitter<readonly DeviceSummary[]>());
	public readonly onDidChangeDevices = this.devicesChanged.event;
	public readonly onDidChangeAvailability = this._register(new Emitter<string>()).event;
	private readonly controlChecked = this._register(new Emitter<ControlCheck>());
	public readonly onDidReceiveControlCheck = this.controlChecked.event;
	private readonly awaitingChanged = this._register(new Emitter<string>());
	public readonly onDidChangeAwaitingAcceptance = this.awaitingChanged.event;
	/** Devices whose person this computer waits on; `setAwaiting` changes it as the engine would. */
	private readonly awaiting = new Set<string>();
	public readonly calls: TestDeviceCall[] = [];
	public incoming: ControlRequestInfo | null = null;
	/** The operating system's name for this computer. */
	public systemName: string | null = 'workstation';

	public isAwaitingAcceptance(id: string): boolean {
		return this.awaiting.has(id);
	}

	/** This computer starts or stops waiting for the person at device `id`. */
	public setAwaiting(id: string, waiting: boolean): void {
		if (waiting) {
			this.awaiting.add(id);
		} else {
			this.awaiting.delete(id);
		}
		this.awaitingChanged.fire(id);
	}

	/** Issues an invitation that lasts fifteen minutes from now. */
	public exportPairingFile = (): Promise<PairingFile> => Promise.resolve({ bytes: [1, 2, 3], check: '042517', expiresAt: Date.now() + 900_000 });

	/** Pairs with the computer `peer`. */
	public pair = (bytes: Uint8Array, digits: string): Promise<string> => {
		this.calls.push({ type: 'pair', bytes: Array.from(bytes), digits });
		return Promise.resolve('peer');
	};

	public incomingPairing(): Promise<ControlRequestInfo | null> {
		return Promise.resolve(this.incoming);
	}

	public systemComputerName(): Promise<string | null> {
		return Promise.resolve(this.systemName);
	}

	public approvePairing(digits: string): Promise<void> {
		this.calls.push({ type: 'approve', digits });
		this.incoming = null;
		return Promise.resolve();
	}

	public rejectPairing(): Promise<void> {
		this.calls.push({ type: 'reject' });
		this.incoming = null;
		return Promise.resolve();
	}

	public connect(id: string): Promise<void> {
		this.calls.push({ type: 'connect', id });
		return Promise.resolve();
	}

	public fireDevices(devices: readonly DeviceSummary[]): void {
		this.devices = devices;
		this.devicesChanged.fire(devices);
	}

	public fireControlCheck(check: ControlCheck): void {
		this.controlChecked.fire(check);
	}

	public availability(): Promise<readonly RemoteCapabilityStatus[] | null> {
		return Promise.resolve(null);
	}

	public initialize(): Promise<void> {
		return Promise.resolve();
	}

	public refresh(): Promise<void> {
		return Promise.resolve();
	}

	public rename(): Promise<void> {
		return Promise.reject(new Error('This test device service renames nothing'));
	}

	public forget(): Promise<void> {
		return Promise.reject(new Error('This test device service forgets nothing'));
	}
}

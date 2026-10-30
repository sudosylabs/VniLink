// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { RunOnceScheduler } from '../../../base/common/async.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IDevicePairingService, type PairingState, type PairingStep } from './devicePairingService.js';
import { IDeviceService, type PairingFile } from './deviceService.js';

/** Invitations larger than this are not VniLink invitations. */
const maxInvitationSize = 65_536;

/** How often a live invitation checks for an incoming pairing request. */
const requestPollInterval = 500;

/** Digits in a pairing confirmation code. */
const codePattern = /^[0-9]{3}$/;

const idle: PairingStep = { kind: 'idle' };

export class DevicePairingService extends Disposable implements IDevicePairingService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<PairingState>());
	public readonly onDidChange = this.changed.event;
	private current: PairingState = { step: idle, busy: false, message: '' };
	private preview: Uint8Array | undefined;
	/** The invitation still waiting for the other computer; a lapsed request returns to it. */
	private offered: PairingFile | undefined;
	/** The live invitation's expiry; zero when none is waiting. */
	private expiresAt = 0;
	/** Bumped by every new flow so late results of an older one are dropped. */
	private revision = 0;
	// Incoming requests have no engine event, so a live invitation polls for them.
	private readonly polling = this._register(new RunOnceScheduler(() => void this.poll(), requestPollInterval));

	constructor(@IDeviceService private readonly devices: IDeviceService) {
		super();
		this._register(this.devices.onDidChangeDevices(known => {
			const step = this.current.step;
			if (step.kind === 'reverseCode' && known.some(device => device.id === step.endpointId && device.presence === 'connected')) {
				this.set({ kind: 'paired' }, nls.localize('devices.paired', 'Computer paired. Your sharing settings still control what it can access.'));
			}
		}));
	}

	public get state(): PairingState {
		return this.current;
	}

	public createInvitation(): Promise<void> {
		return this.run(async revision => {
			this.preview = undefined;
			const file = await this.devices.exportPairingFile();
			if (!this.isCurrent(revision)) {
				return;
			}
			this.offered = file;
			this.expiresAt = file.expiresAt;
			this.set({ kind: 'invitation', file });
			this.polling.schedule();
		}, true);
	}

	public openInvitation(file: File): Promise<void> {
		return this.run(async revision => {
			this.stopWaiting();
			if (file.size > maxInvitationSize || !file.size) {
				throw new Error(nls.localize('devices.fileSize', 'Choose a valid .vnl invitation smaller than 64 KB.'));
			}
			const bytes = new Uint8Array(await file.arrayBuffer());
			if (!this.isCurrent(revision)) {
				return;
			}
			this.preview = bytes;
			this.set({ kind: 'opened', fileName: file.name });
		}, true);
	}

	public submitCode(digits: string): Promise<void> {
		if (!codePattern.test(digits)) {
			this.setMessage(nls.localize('devices.codeFormat', 'Enter the three digits shown on the other computer.'));
			return Promise.resolve();
		}
		const step = this.current.step;
		if (step.kind === 'opened' && this.preview) {
			const preview = this.preview;
			return this.run(async revision => {
				const endpointId = await this.devices.pair(preview, digits);
				const code = await this.devices.pairingCheck();
				if (!this.isCurrent(revision)) {
					return;
				}
				this.preview = undefined;
				this.set({ kind: 'reverseCode', code, endpointId });
				await this.devices.connect(endpointId);
			});
		}
		if (step.kind === 'approval') {
			return this.run(async revision => {
				const request = await this.devices.incomingPairing();
				if (request?.endpoint_id !== step.endpointId) {
					throw new Error(nls.localize('devices.requestChanged', 'The pairing request expired or changed. Wait for the other computer to try again.'));
				}
				await this.devices.approvePairing(digits);
				if (this.isCurrent(revision)) {
					this.stopWaiting();
					this.set({ kind: 'paired' }, nls.localize('devices.paired', 'Computer paired. Your sharing settings still control what it can access.'));
				}
			});
		}
		return Promise.resolve();
	}

	public reject(): Promise<void> {
		if (this.current.step.kind !== 'approval') {
			return Promise.resolve();
		}
		return this.run(async revision => {
			await this.devices.rejectPairing();
			if (this.isCurrent(revision)) {
				this.stopWaiting();
				this.set({ kind: 'ended' }, nls.localize('devices.rejected', 'Pairing request rejected.'));
			}
		});
	}

	public reset(): void {
		const kind = this.current.step.kind;
		// A live invitation or a pending request is still waiting on the other computer.
		if (this.current.busy || kind === 'invitation' || kind === 'approval') {
			return;
		}
		this.revision++;
		this.preview = undefined;
		this.set(idle);
	}

	private async poll(): Promise<void> {
		if (!this.expiresAt || this.isDisposed()) {
			return;
		}
		if (Date.now() >= this.expiresAt) {
			this.stopWaiting();
			this.set({ kind: 'ended' }, nls.localize('devices.expired', 'The invitation expired. Create a new invitation to pair.'));
			return;
		}
		const step = this.current.step;
		try {
			const request = await this.devices.incomingPairing();
			if (this.isDisposed() || this.current.busy || this.current.step !== step) {
				return;
			}
			if (request && (step.kind !== 'approval' || request.endpoint_id !== step.endpointId)) {
				this.set({ kind: 'approval', endpointId: request.endpoint_id });
			} else if (!request && step.kind === 'approval' && this.offered) {
				// The other computer stopped asking, but it redials while the invitation lasts.
				this.set({ kind: 'invitation', file: this.offered }, nls.localize('devices.requestLapsed', 'The other computer stopped waiting. It will ask again while this invitation lasts.'));
			}
		} catch {
			if (!this.isDisposed()) {
				this.setMessage(nls.localize('devices.approvalUnavailable', 'Unable to check incoming pairing requests. Retrying…'));
			}
		} finally {
			if (!this.isDisposed() && this.expiresAt) {
				this.polling.schedule();
			}
		}
	}

	/** Re-read after every await: the window can close while a request is in flight. */
	private isDisposed(): boolean {
		return this._store.isDisposed;
	}

	private stopWaiting(): void {
		this.offered = undefined;
		this.expiresAt = 0;
		this.polling.cancel();
	}

	private isCurrent(revision: number): boolean {
		return !this.isDisposed() && this.revision === revision;
	}

	/** Runs one command; `fresh` starts a new flow, so earlier results are dropped. */
	private async run(command: (revision: number) => Promise<void>, fresh = false): Promise<void> {
		if (this.current.busy || this.isDisposed()) {
			return;
		}
		const revision = fresh ? ++this.revision : this.revision;
		this.update({ busy: true, message: '' });
		try {
			await command(revision);
		} catch (error) {
			if (this.isCurrent(revision)) {
				this.setMessage(error instanceof Error ? error.message : nls.localize('devices.pairingFailed', 'Pairing failed. Try again.'));
			}
		} finally {
			if (!this.isDisposed()) {
				this.update({ busy: false });
			}
		}
	}

	private set(step: PairingStep, message = ''): void {
		this.update({ step, message });
	}

	private setMessage(message: string): void {
		this.update({ message });
	}

	private update(change: Partial<PairingState>): void {
		if (this.isDisposed()) {
			return;
		}
		this.current = { ...this.current, ...change };
		this.changed.fire(this.current);
	}
}

registerSingleton(IDevicePairingService, DevicePairingService, InstantiationType.Delayed);

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter, type Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';
import { IMainProcessService } from '../../ipc/common/mainProcessService.js';
import type { DataChannelRequest } from '../common/dataChannel.js';
import { isEngineReply, type EngineCommand, type EngineReply } from '../common/engine.js';
import { ENGINE_CHANNEL_NAME, type EngineEventBatch, type IEngineService } from '../common/engineService.js';

/** Window side of the engine channel; keeps availability and restart state current. */
export class EngineService extends Disposable implements IEngineService {
	declare public readonly _serviceBrand: undefined;
	private readonly channel: IChannel;
	private readonly availabilityChanged = this._register(new Emitter<boolean>());
	public readonly onDidChangeAvailability = this.availabilityChanged.event;
	private readonly restartingChanged = this._register(new Emitter<boolean>());
	public readonly onDidChangeRestarting = this.restartingChanged.event;
	private readonly restartRequiredChanged = this._register(new Emitter<boolean>());
	public readonly onDidChangeRestartRequired = this.restartRequiredChanged.event;
	public readonly onDidReceiveEvents: Event<EngineEventBatch>;
	private _available: boolean;
	private _restartRequired = false;
	private _restarting = false;

	constructor(
		available: boolean,
		@IMainProcessService mainProcessService: IMainProcessService,
	) {
		super();
		this._available = available;
		this.channel = mainProcessService.getChannel(ENGINE_CHANNEL_NAME);
		this.onDidReceiveEvents = this.channel.listen<EngineEventBatch>('onDidReceiveEvents');
		this._register(this.channel.listen<boolean>('onDidChangeAvailability')(value => {
			this.setAvailable(value);
		}));
		this._register(this.channel.listen<boolean>('onDidChangeRestarting')(value => {
			this.setRestarting(value);
		}));
		this._register(this.channel.listen<boolean>('onDidChangeRestartRequired')(value => {
			this.setRestartRequired(value);
		}));
		if (available) {
			this.readRestartRequired();
		} else {
			// A window opened while the engine restarts.
			void this.channel.call<boolean>('getRestarting').then(value => {
				this.setRestarting(value);
			}, () => { /* Without an answer the window shows the engine as given up. */ });
		}
	}

	/** A restarted engine starts from the saved relays, so the requirement is read again. */
	private readRestartRequired(): void {
		void this.channel.call<boolean>('getRestartRequired').then(value => {
			this.setRestartRequired(value);
		}, () => { /* An unavailable engine cannot report restart requirements. */ });
	}

	public get restarting(): boolean {
		return this._restarting;
	}

	public get available(): boolean {
		return this._available;
	}

	public get restartRequired(): boolean {
		return this._restartRequired;
	}

	public async execute(command: EngineCommand): Promise<EngineReply> {
		const reply = await this.channel.call<unknown>('execute', command);
		if (!isEngineReply(reply)) {
			throw new Error('Invalid engine reply');
		}
		return reply;
	}

	public openDataChannel(request: DataChannelRequest): Promise<void> {
		return this.channel.call('openDataChannel', request);
	}

	private setAvailable(value: boolean): void {
		if (!this._store.isDisposed && value !== this._available) {
			this._available = value;
			this.availabilityChanged.fire(value);
			if (value) {
				this.readRestartRequired();
			}
		}
	}

	private setRestarting(value: boolean): void {
		if (!this._store.isDisposed && value !== this._restarting) {
			this._restarting = value;
			this.restartingChanged.fire(value);
		}
	}

	private setRestartRequired(value: boolean): void {
		if (!this._store.isDisposed && value !== this._restartRequired) {
			this._restartRequired = value;
			this.restartRequiredChanged.fire(value);
		}
	}
}

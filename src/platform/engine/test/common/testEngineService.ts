// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter } from '../../../../base/common/event.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import type { EngineCommand, EngineReply } from '../../common/engine.js';
import type { EngineEventBatch, IEngineService } from '../../common/engineService.js';

export type EngineSnapshotReply = Extract<EngineReply, { type: 'snapshot' }>;

export type EngineDevice = EngineSnapshotReply['snapshot']['devices'][number];

/** A paired device as the engine reports it; `overrides` set what a test cares about. */
export function engineDevice(id: string, overrides: Partial<EngineDevice> = {}): EngineDevice {
	return {
		endpoint_id: id, pet_name: null, hostname: null, os: 'linux', presence: 'connected', direct: [], relays: [],
		grants: { view: false, control: false, clipboard: false, transfer: false },
		...overrides,
	};
}

/** The engine's snapshot reply listing `devices`, on a computer with no session and no relay. */
export function snapshotReply(devices: readonly EngineDevice[]): EngineSnapshotReply {
	return {
		type: 'snapshot',
		snapshot: {
			endpoint_id: 'this-computer', hosting: false, public_relay: false, relay_urls: [], devices: [...devices], incoming: null, outgoing: null,
			capabilities: {
				desktop_capture: false, keyboard: false, pointer: false, clipboard: false, file_transfer: false,
				audio: false, monitors: 0, h264_encode: false, h264_decode: false,
			},
		},
	};
}

/**
 * An {@link IEngineService} whose replies a test scripts: `handler` answers
 * each command (a throw is a refusal), `fireEvents` delivers an event batch and `setAvailable`
 * changes availability and `setRestartRequired` the restart state, as main
 * would report them. Every command it was
 * asked is kept in `commands`.
 */
export class TestEngineService extends Disposable implements IEngineService {
	declare public readonly _serviceBrand: undefined;
	private readonly availabilityChanged = this._register(new Emitter<boolean>());
	public readonly onDidChangeAvailability = this.availabilityChanged.event;
	public readonly restarting = false;
	public readonly onDidChangeRestarting = this._register(new Emitter<boolean>()).event;
	private readonly restartRequiredChanged = this._register(new Emitter<boolean>());
	public readonly onDidChangeRestartRequired = this.restartRequiredChanged.event;
	private readonly events = this._register(new Emitter<EngineEventBatch>());
	public readonly onDidReceiveEvents = this.events.event;
	public readonly commands: EngineCommand[] = [];

	constructor(
		public handler: (command: EngineCommand) => EngineReply | Promise<EngineReply>,
		private _available = true,
	) {
		super();
	}

	public get available(): boolean {
		return this._available;
	}

	private _restartRequired = false;

	public get restartRequired(): boolean {
		return this._restartRequired;
	}

	/** Whether anyone still listens for engine events. */
	public get hasEventListeners(): boolean {
		return this.events.hasListeners();
	}

	public execute(command: EngineCommand): Promise<EngineReply> {
		this.commands.push(command);
		// A handler that throws is an engine that refused the command: the caller sees a rejection.
		return new Promise(resolve => {
			resolve(this.handler(command));
		});
	}

	public openDataChannel(): Promise<void> {
		return Promise.reject(new Error('This test engine opens no data channels'));
	}

	public fireEvents(batch: EngineEventBatch): void {
		this.events.fire(batch);
	}

	public setAvailable(available: boolean): void {
		this._available = available;
		this.availabilityChanged.fire(available);
	}

	public setRestartRequired(restartRequired: boolean): void {
		this._restartRequired = restartRequired;
		this.restartRequiredChanged.fire(restartRequired);
	}
}

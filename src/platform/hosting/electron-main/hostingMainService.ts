// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { IConfigurationMainService } from '../../configuration/electron-main/configurationMainService.js';
import type { ConfigurationMainService } from '../../configuration/electron-main/configurationMainService.js';
import { deviceName, type EngineEvent, type SessionPhase } from '../../engine/common/engine.js';
import type { EngineEventBatch } from '../../engine/common/engineService.js';
import { IEngineMainService } from '../../engine/electron-main/engineMainService.js';
import type { EngineMainService } from '../../engine/electron-main/engineMainService.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';

export const IHostingMainService = createDecorator<HostingMainService>('hostingMainService');

/** A paired device connected to this computer. */
export interface HostedPeer {
	readonly id: string;
	readonly name: string;
	readonly phase: SessionPhase;
}

/** Whether this computer accepts paired devices, and who is connected to it. */
export interface HostingStatus {
	/** Whether the engine answers commands. */
	readonly available: boolean;
	/** The owner's choice, `application.acceptConnections`; it survives restarts. */
	readonly accepting: boolean;
	/** Whether the engine is hosting now. */
	readonly hosting: boolean;
	readonly sessions: readonly HostedPeer[];
}

export const ACCEPT_CONNECTIONS_SETTING = 'application.acceptConnections';

const statusEvents = new Set<EngineEvent['type']>(['hostingChanged', 'sessionChanged', 'hostedServiceChanged', 'devicePaired', 'deviceUnpaired', 'peerRenamed']);

/**
 * Owns whether this computer is reachable: it hosts while the application
 * runs, with or without a window, unless the owner paused sharing, and
 * follows the engine's events so main-process surfaces can show who is
 * connected. Rust still gates every service with global consent and unknown
 * peers with owner confirmation.
 */
export class HostingMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly statusChanged = this._register(new Emitter<HostingStatus>());
	public readonly onDidChangeStatus = this.statusChanged.event;
	private current: HostingStatus;
	private started = false;
	/** Set when the last attempt to match `accepting` failed, so the next resync tries again. */
	private applyFailed = false;
	private refreshing: Promise<void> | undefined;
	private refreshAgain = false;

	constructor(
		@IEngineMainService private readonly engine: EngineMainService,
		@IConfigurationMainService private readonly configuration: ConfigurationMainService,
		@ILogService private readonly logService: ILogService,
	) {
		super();
		this.current = { available: false, accepting: this.accepting, hosting: false, sessions: [] };
	}

	public get status(): HostingStatus {
		return this.current;
	}

	private get accepting(): boolean {
		return this.configuration.getValue(ACCEPT_CONNECTIONS_SETTING) !== false;
	}

	/** Matches hosting to the owner's choice and listens to engine events until the application quits. */
	public start(): void {
		if (this.started) {
			return;
		}
		this.started = true;
		// Listening keeps the engine host polling while no window is open.
		this._register(this.engine.onDidReceiveEvents(batch => {
			this.onEvents(batch);
		}));
		this._register(this.engine.onDidChangeAvailability(available => {
			if (available) {
				void this.apply();
			} else {
				this.update({ available: false, accepting: this.accepting, hosting: false, sessions: [] });
			}
		}));
		this._register(this.configuration.onDidChangeConfiguration(() => {
			if (this.accepting !== this.current.accepting) {
				void this.apply();
			}
		}));
		if (this.engine.available) {
			void this.apply();
		}
	}

	/** Pauses or resumes sharing; the choice is saved and outlives this run. */
	public async setAccepting(accepting: boolean): Promise<void> {
		await this.configuration.updateValue(ACCEPT_CONNECTIONS_SETTING, accepting);
	}

	/** Ends one connected device's session; other devices and hosting stay up. */
	public async disconnect(sessionId: string): Promise<void> {
		await this.engine.execute({ type: 'stopHostedPeer', sessionId });
		await this.refresh();
	}

	private async apply(): Promise<void> {
		const accepting = this.accepting;
		try {
			await this.engine.execute({ type: accepting ? 'startHosting' : 'stopHosting' });
			this.applyFailed = false;
		} catch (error) {
			this.applyFailed = true;
			this.logService.error(accepting ? 'This computer could not start accepting paired devices.' : 'This computer could not stop accepting paired devices.', error);
		}
		await this.refresh();
	}

	private onEvents(batch: EngineEventBatch): void {
		if (batch.resync && (this.applyFailed || this.current.hosting !== this.accepting)) {
			void this.apply();
		} else if (batch.resync || batch.events.some(event => statusEvents.has(event.type))) {
			void this.refresh();
		}
	}

	/** Reads the engine's state; events during a read schedule one more read rather than overlapping. */
	private refresh(): Promise<void> {
		if (this.refreshing) {
			this.refreshAgain = true;
			return this.refreshing;
		}
		this.refreshing = (async () => {
			do {
				this.refreshAgain = false;
				await this.read();
			} while (this.shouldRefreshAgain());
		})().finally(() => {
			this.refreshing = undefined;
		});
		return this.refreshing;
	}

	private async read(): Promise<void> {
		try {
			const [snapshot, hosted, services] = await Promise.all([
				this.engine.execute({ type: 'snapshot' }), this.engine.execute({ type: 'hostedSessions' }), this.engine.execute({ type: 'hostedServices' }),
			]);
			if (snapshot.type !== 'snapshot' || hosted.type !== 'sessions' || services.type !== 'services') {
				return;
			}
			const devices = snapshot.snapshot.devices;
			// A paired computer keeps an idle link to this one to know it is reachable;
			// only a session carrying a service means someone is using this computer.
			const inUse = new Set(services.services.filter(service => typeof service.phase === 'string').map(service => service.connectionId));
			const sessions = hosted.sessions.filter(session => session.phase !== 'Stopping' && inUse.has(session.id)).map(session => {
				const device = devices.find(candidate => candidate.endpoint_id === session.peer_id);
				return { id: session.id, name: device ? deviceName(device) : session.peer_id, phase: session.phase };
			});
			this.update({ available: true, accepting: this.accepting, hosting: snapshot.snapshot.hosting, sessions });
		} catch {
			// Unavailability is reported through `onDidChangeAvailability`.
		}
	}

	/** Read separately: `refreshAgain` changes while a read is awaited. */
	private shouldRefreshAgain(): boolean {
		return this.refreshAgain && !this._store.isDisposed;
	}

	private update(status: HostingStatus): void {
		if (this._store.isDisposed || JSON.stringify(status) === JSON.stringify(this.current)) {
			return;
		}
		this.current = status;
		this.statusChanged.fire(status);
	}
}

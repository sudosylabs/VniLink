// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { app, powerMonitor } from 'electron';
import { Emitter, Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { isLinux } from '../../../base/common/platform.js';
import { isBuilt } from '../../environment/electron-main/environment.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';

export const ILifecycleMainService = createDecorator<LifecycleMainService>('lifecycleMainService');

interface ShutdownEvent {
	/** Delays quitting until `promise` settles. Failures are logged, never block shutdown. */
	join(id: string, promise: Promise<unknown>): void;
}

/** How long quitting waits for the participants; one that hangs does not keep the process alive. */
const shutdownDeadline = 20_000;

/** Exit code that asks the development runner to start the application again. */
const DEVELOPMENT_RELAUNCH_EXIT_CODE = 42;

/**
 * Owns quitting and relaunching. Services that hold external resources join
 * {@link onWillShutdown} so the engine and settings close before the process exits.
 */
export class LifecycleMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly willShutdown = this._register(new Emitter<ShutdownEvent>());
	public readonly onWillShutdown = this.willShutdown.event;
	private quitRequested = false;
	private shutdownComplete = false;
	private relaunching = false;
	private shutdown: Promise<void> | undefined;

	constructor(@ILogService private readonly logService: ILogService) {
		super();
		this._register(Event.fromNodeEventEmitter<Electron.Event>(app, 'before-quit')(event => {
			this.onBeforeQuit(event);
		}));
		// Windows logging off or shutting down emits this instead of `before-quit`.
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'session-end')(() => {
			void this.runShutdown();
		}));
	}

	/**
	 * Quits when Linux is about to shut down, holding the system back until
	 * the participants are done. macOS asks the application to quit itself;
	 * Windows ends the session (see the constructor). Call once the app is ready.
	 */
	public listenForSystemShutdown(): void {
		if (!isLinux) {
			return;
		}
		this._register(Event.fromNodeEventEmitter<Electron.Event>(powerMonitor, 'shutdown')(event => {
			event.preventDefault();
			app.quit();
		}));
	}

	public get quitting(): boolean {
		return this.quitRequested;
	}

	/** Restarts the application. In development the runner restarts it after exit. */
	public relaunch(): void {
		if (this.relaunching) {
			return;
		}
		this.relaunching = true;
		if (!this.isDevelopmentRunner()) {
			app.relaunch();
		}
		app.quit();
	}

	/** Runs shutdown participants once; used directly when startup fails. */
	public runShutdown(): Promise<void> {
		this.quitRequested = true;
		this.shutdown ??= this.joinParticipants();
		return this.shutdown;
	}

	private onBeforeQuit(event: Electron.Event): void {
		if (this.shutdownComplete) {
			return;
		}
		event.preventDefault();
		if (this.shutdown) {
			return;
		}
		void this.runShutdown().finally(() => {
			this.shutdownComplete = true;
			if (this.relaunching && this.isDevelopmentRunner()) {
				app.exit(DEVELOPMENT_RELAUNCH_EXIT_CODE);
			} else {
				app.quit();
			}
		});
	}

	private async joinParticipants(): Promise<void> {
		const participants: { id: string; promise: Promise<unknown> }[] = [];
		this.willShutdown.fire({ join: (id, promise) => participants.push({ id, promise }) });
		const pending = new Set(participants.map(participant => participant.id));
		const settled = participants.map(({ id, promise }) => promise.then(() => {
			pending.delete(id);
		}, (error: unknown) => {
			pending.delete(id);
			this.logService.error(`Shutdown participant ${id} failed`, error);
		}));
		let timer: ReturnType<typeof setTimeout> | undefined;
		const deadline = new Promise<void>(resolve => {
			timer = setTimeout(resolve, shutdownDeadline);
		});
		await Promise.race([Promise.all(settled), deadline]);
		clearTimeout(timer);
		if (pending.size) {
			this.logService.error(`Quitting without waiting longer for ${[...pending].join(', ')}, still running after ${String(shutdownDeadline / 1000)} seconds`);
		}
	}

	private isDevelopmentRunner(): boolean {
		return !isBuilt() && process.env.VNILINK_DEV === '1';
	}
}

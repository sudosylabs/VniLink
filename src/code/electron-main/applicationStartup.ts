// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Event } from '../../base/common/event.js';
import type { IDisposable } from '../../base/common/lifecycle.js';
import type { ShutdownEvent } from '../../platform/lifecycle/electron-main/lifecycleMainService.js';
import type { ILogService } from '../../platform/log/common/log.js';

/**
 * Whether closing the window leaves the application running: the owner's
 * `application.runInBackground`, unless this launch restricts it because no
 * tray icon would show.
 */
export function runsInBackground(restricted: readonly string[], value: unknown): boolean {
	return !restricted.includes('application.runInBackground') && value !== false;
}

/**
 * Whether startup hosts without opening a window: an installed engine that
 * runs in the background and was started by the login item. `launchedAtLogin`
 * is asked only when the rest holds.
 */
export function startsWithoutWindow(temporaryEngine: boolean, background: boolean, launchedAtLogin: () => boolean): boolean {
	return !temporaryEngine && background && launchedAtLogin();
}

/**
 * Whether the application shares this screen with paired computers. A
 * temporary engine shares it only when it may capture; on Linux asking shows
 * the desktop portal's dialog.
 */
export function sharesScreen(options: { readonly temporaryEngine?: boolean; readonly temporaryEngineCaptures?: boolean }): boolean {
	return !options.temporaryEngine || options.temporaryEngineCaptures === true;
}

/** What quitting closes before the process exits. */
interface ShutdownParticipants {
	readonly engine: { stop(): Promise<void> };
	readonly configuration: { flush(): Promise<void> };
	readonly state: { close(): Promise<void> };
}

/** Quitting waits for the engine to stop and for settings and UI state to be saved. */
export function joinShutdown(onWillShutdown: Event<ShutdownEvent>, { engine, configuration, state }: ShutdownParticipants): IDisposable {
	return onWillShutdown(event => {
		event.join('engine', engine.stop());
		event.join('configuration', configuration.flush());
		event.join('state', state.close());
	});
}

/** The windows service as window requests use it: whether the main window is open, and focusing it. */
interface RequestedWindows {
	getMainWindow(): object | undefined;
	focusMainWindow(): void;
}

/**
 * Answers each request to see the window (a second launch, activation, the
 * tray's Open) with one window: an open window is focused, and requests
 * while one opens share it. A request made before startup has decided whether
 * to open a window waits for that decision.
 */
export class MainWindowRequests {
	/** Whether startup has opened its window or decided to start without one. */
	private ready = false;
	/** A request that arrived before startup was ready. */
	private requested = false;
	private opening: Promise<void> | undefined;

	constructor(
		private readonly windows: RequestedWindows,
		private readonly lifecycle: { readonly quitting: boolean },
		private readonly openWindow: () => Promise<unknown>,
		private readonly logService: ILogService,
	) { }

	/** Focuses the window, or opens it when the application runs without one. */
	public request(): void {
		if (this.lifecycle.quitting) {
			return;
		}
		if (!this.ready) {
			this.requested = true;
			return;
		}
		if (this.windows.getMainWindow()) {
			this.windows.focusMainWindow();
			return;
		}
		this.opening ??= this.openWindow().then(() => undefined, (error: unknown) => {
			this.logService.error(error instanceof Error ? error : new Error(String(error)));
		}).finally(() => {
			this.opening = undefined;
		});
	}

	/** Startup opened the window, which answers the requests made while it started. */
	public openedAtStartup(): void {
		this.ready = true;
	}

	/** Startup hosts without a window; a request made while it started opens one now. */
	public startedWithoutWindow(): void {
		this.ready = true;
		if (this.requested) {
			this.request();
		}
	}
}

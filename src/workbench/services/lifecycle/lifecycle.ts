/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: moved out of common/ beside its implementation; its relative imports follow.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { CancellationToken } from '../../../base/common/cancellation.js';
import { Event } from '../../../base/common/event.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

export const ILifecycleService = createDecorator<ILifecycleService>('lifecycleService');

/**
 * An event that is send out when the window is about to close. Clients have a chance to veto
 * the closing by either calling veto with a boolean "true" directly or with a promise that
 * resolves to a boolean. Returning a promise is useful in cases of long running operations
 * on shutdown.
 *
 * Note: It is absolutely important to avoid long running promises if possible. Please try hard
 * to return a boolean directly. Returning a promise has quite an impact on the shutdown sequence!
 */
export interface BeforeShutdownEvent {

	/**
	 * The reason why the application will be shutting down.
	 */
	readonly reason: ShutdownReason;
}

/**
 * An event that signals an error happened during `onBeforeShutdown` veto handling.
 * In this case the shutdown operation will not proceed because this is an unexpected
 * condition that is treated like a veto.
 */
export interface BeforeShutdownErrorEvent {

	/**
	 * The reason why the application is shutting down.
	 */
	readonly reason: ShutdownReason;

	/**
	 * The error that happened during shutdown handling.
	 */
	readonly error: Error;
}

/**
 * An event that is send out when the window closes. Clients have a chance to join the closing
 * by providing a promise from the join method. Returning a promise is useful in cases of long
 * running operations on shutdown.
 *
 * Note: It is absolutely important to avoid long running promises if possible. Please try hard
 * to return a boolean directly. Returning a promise has quite an impact on the shutdown sequence!
 */
export interface WillShutdownEvent {

	/**
	 * The reason why the application is shutting down.
	 */
	readonly reason: ShutdownReason;

	/**
	 * A token that will signal cancellation when the
	 * shutdown was forced by the user.
	 */
	readonly token: CancellationToken;
}

export const enum ShutdownReason {

	/**
	 * The window is closed.
	 */
	CLOSE = 1,

	/**
	 * The window closes because the application quits.
	 */
	QUIT,

	/**
	 * The window is reloaded.
	 */
	RELOAD,

	/**
	 * The window is loaded into a different workspace context.
	 */
	LOAD
}

export const enum StartupKind {
	NewWindow = 1,
	ReloadedWindow = 3,
	ReopenedWindow = 4
}

export const enum LifecyclePhase {

	/**
	 * The first phase signals that we are about to startup getting ready.
	 *
	 * Note: doing work in this phase blocks an editor from showing to
	 * the user, so please rather consider to use `Restored` phase.
	 */
	Starting = 1,

	/**
	 * Services are ready and the window is about to restore its UI state.
	 *
	 * Note: doing work in this phase blocks an editor from showing to
	 * the user, so please rather consider to use `Restored` phase.
	 */
	Ready = 2,

	/**
	 * Views, panels and editors have restored. Editors are given a bit of
	 * time to restore their contents.
	 */
	Restored = 3,

	/**
	 * The last phase after views, panels and editors have restored and
	 * some time has passed (2-5 seconds).
	 */
	Eventually = 4
}

/**
 * A lifecycle service informs about lifecycle events of the
 * application, such as shutdown.
 */
export interface ILifecycleService {

	readonly _serviceBrand: undefined;

	/**
	 * Value indicates how this window got loaded.
	 */
	readonly startupKind: StartupKind;

	/**
	 * A flag indicating in what phase of the lifecycle we currently are.
	 */
	phase: LifecyclePhase;

	/**
	 * Fired before shutdown happens. Allows listeners to veto against the
	 * shutdown to prevent it from happening.
	 *
	 * The event carries a shutdown reason that indicates how the shutdown was triggered.
	 */
	readonly onBeforeShutdown: Event<BeforeShutdownEvent>;

	/**
	 * Fired when the shutdown was prevented by a component giving veto.
	 */
	readonly onShutdownVeto: Event<void>;

	/**
	 * Fired when an error happened during `onBeforeShutdown` veto handling.
	 * In this case the shutdown operation will not proceed because this is
	 * an unexpected condition that is treated like a veto.
	 *
	 * The event carries a shutdown reason that indicates how the shutdown was triggered.
	 */
	readonly onBeforeShutdownError: Event<BeforeShutdownErrorEvent>;

	/**
	 * Fired when no client is preventing the shutdown from happening (from `onBeforeShutdown`).
	 *
	 * This event can be joined with a long running operation via `WillShutdownEvent#join()` to
	 * handle long running shutdown operations.
	 *
	 * The event carries a shutdown reason that indicates how the shutdown was triggered.
	 */
	readonly onWillShutdown: Event<WillShutdownEvent>;

	/**
	 * A flag indicating that we are about to shutdown without further veto.
	 */
	readonly willShutdown: boolean;

	/**
	 * Fired when the shutdown is about to happen after long running shutdown operations
	 * have finished (from `onWillShutdown`).
	 *
	 * This event should be used to dispose resources.
	 */
	readonly onDidShutdown: Event<void>;

	/**
	 * Returns a promise that resolves when a certain lifecycle phase
	 * has started.
	 */
	when(phase: LifecyclePhase): Promise<void>;

	/**
	 * Triggers a shutdown of the workbench. Depending on native or web, this can have
	 * different implementations and behaviour.
	 *
	 * **Note:** this should normally not be called. See related methods in `IHostService`
	 * and `INativeHostService` to close a window or quit the application.
	 */
	shutdown(): Promise<void>;
}

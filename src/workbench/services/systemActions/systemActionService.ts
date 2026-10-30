// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Event } from '../../../base/common/event.js';
import type { SystemActionKind, SystemPrivacy } from '../../../platform/engine/common/engine.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

export const ISystemActionService = createDecorator<ISystemActionService>('systemActionService');

/** Where a lock, restart or shutdown asked of one computer stands, in words. */
export interface SystemActionProgress {
	readonly action: SystemActionKind;
	/** `scheduled` while a restart or shutdown counts down and can be cancelled. */
	readonly phase: 'asking' | 'scheduled' | 'ended';
	readonly message: string;
}

/** Privacy mode as asked of one computer: where it stands, or why it couldn't be set, in words. */
export type SystemPrivacyResult = { readonly privacy: SystemPrivacy } | { readonly error: string };

/**
 * Locks, restarts and shuts down paired computers, and blanks their screen
 * and blocks their keyboard and mouse while privacy mode is on. A restart or shutdown
 * waits out a notice on that computer, where its user can cancel it; this
 * computer can cancel it too until then.
 */
export interface ISystemActionService {
	readonly _serviceBrand: undefined;
	/** Fires with the device id whose progress changed. */
	readonly onDidChange: Event<string>;
	progress(deviceId: string): SystemActionProgress | undefined;
	run(deviceId: string, action: SystemActionKind): Promise<void>;
	/** Cancels the restart or shutdown counting down on `deviceId`. */
	cancel(deviceId: string): Promise<void>;
	/** Privacy mode on `deviceId` as it stands. */
	privacy(deviceId: string): Promise<SystemPrivacyResult>;
	/**
	 * Turns privacy mode on `deviceId` on or off. One computer holds it at a
	 * time; it ends by itself when this computer disconnects.
	 */
	setPrivacy(deviceId: string, enabled: boolean): Promise<SystemPrivacyResult>;
}

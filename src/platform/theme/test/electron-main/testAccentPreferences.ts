// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { EventEmitter } from 'events';
import type { AccentPreferences } from '../../electron-main/systemAccent.js';

/** What macOS hands a notification's subscriber. */
type NotificationCallback = (event: string, userInfo: Record<string, unknown>, object: string) => void;

/**
 * Electron's `systemPreferences` as a test sets it: `accent` is what
 * `getAccentColor` answers, and `post` delivers a macOS notification.
 */
export class TestAccentPreferences extends EventEmitter implements AccentPreferences {
	public accent = '0a84ffff';
	/** Subscribed notifications by id, shared across applications or local to this one. */
	public readonly subscriptions = new Map<number, { readonly event: string | null; readonly local: boolean; readonly callback: NotificationCallback }>();
	private nextId = 1;

	public getAccentColor(): string {
		return this.accent;
	}

	public subscribeNotification(event: string | null, callback: NotificationCallback): number {
		return this.subscribe(event, false, callback);
	}

	public unsubscribeNotification(id: number): void {
		this.subscriptions.delete(id);
	}

	public subscribeLocalNotification(event: string | null, callback: NotificationCallback): number {
		return this.subscribe(event, true, callback);
	}

	public unsubscribeLocalNotification(id: number): void {
		this.subscriptions.delete(id);
	}

	public post(event: string): void {
		for (const subscription of [...this.subscriptions.values()].filter(candidate => candidate.event === event)) {
			subscription.callback(event, {}, '');
		}
	}

	private subscribe(event: string | null, local: boolean, callback: NotificationCallback): number {
		const id = this.nextId++;
		this.subscriptions.set(id, { event, local, callback });
		return id;
	}
}

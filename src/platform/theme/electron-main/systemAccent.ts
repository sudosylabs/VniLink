// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { systemPreferences } from 'electron';
import type { SystemPreferences } from 'electron';
import { Emitter, Event } from '../../../base/common/event.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { windowPlatform } from '../../window/common/windowMaterial.js';

/** The part of Electron's `systemPreferences` the accent reads; a test gives its own. */
export type AccentPreferences = Event.NodeEventEmitter & Pick<SystemPreferences, 'getAccentColor' | 'subscribeNotification' | 'unsubscribeNotification' | 'subscribeLocalNotification' | 'unsubscribeLocalNotification'>;

/**
 * The accent colour the user chose for the system, as `#rrggbb`, and its
 * changes. macOS and Windows have one; elsewhere it is `undefined` and the
 * palette keeps the product accent.
 */
export class SystemAccent extends Disposable {
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChange = this.changed.event;
	private _color: string | undefined;

	constructor(
		private readonly preferences: AccentPreferences = systemPreferences,
		private readonly platform: string = windowPlatform,
	) {
		super();
		this._color = this.read();
		const update = (): void => {
			const color = this.read();
			if (color !== this._color) {
				this._color = color;
				this.changed.fire();
			}
		};
		if (platform === 'win32') {
			this._register(Event.fromNodeEventEmitter<unknown>(preferences, 'accent-color-changed')(update));
		} else if (platform === 'darwin') {
			// Posted across applications when the user picks another accent, and
			// inside this one when the system colours that follow it change.
			const shared = preferences.subscribeNotification('AppleColorPreferencesChangedNotification', update);
			const local = preferences.subscribeLocalNotification('NSSystemColorsDidChangeNotification', update);
			this._register(toDisposable(() => {
				preferences.unsubscribeNotification(shared);
				preferences.unsubscribeLocalNotification(local);
			}));
		}
	}

	public get color(): string | undefined {
		return this._color;
	}

	private read(): string | undefined {
		if (this.platform !== 'darwin' && this.platform !== 'win32') {
			return undefined;
		}
		try {
			// Electron answers `RRGGBBAA`; an accent is opaque.
			const match = /^([\da-f]{6})(?:[\da-f]{2})?$/i.exec(this.preferences.getAccentColor());
			return match ? `#${match[1].toLowerCase()}` : undefined;
		} catch {
			// A system that cannot answer has no accent to follow.
			return undefined;
		}
	}
}

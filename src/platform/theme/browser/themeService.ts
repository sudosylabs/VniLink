/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: semantic workbench colors, native titlebar presentation and the system accent.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { Color } from '../../../base/common/color.js';
import { onUnexpectedError } from '../../../base/common/errors.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { Emitter } from '../../../base/common/event.js';
import { INativeHostService } from '../../native/common/native.js';
import { IThemeService, type ITitlebarTheme } from '../common/themeService.js';
import { registerSingleton, InstantiationType } from '../../instantiation/common/extensions.js';

export class ThemeService extends Disposable implements IThemeService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<ITitlebarTheme>());
	public readonly onDidColorThemeChange = this.changed.event;
	private readonly media = window.matchMedia('(prefers-color-scheme: dark)');
	private readonly contrast = window.matchMedia('(forced-colors: active)');
	private readonly paletteProbe = document.createElement('span');
	/** The system's accent, as the main process last reported it. */
	private systemAccent: string | undefined;

	constructor(@INativeHostService private readonly nativeHostService: INativeHostService) {
		super();
		this.paletteProbe.hidden = true;
		this.paletteProbe.style.forcedColorAdjust = 'none';
		document.documentElement.append(this.paletteProbe);
		this._register(toDisposable(() => {
			this.paletteProbe.remove();
		}));
		const listener = (): void => {
			this.applySystemAccent();
			this.changed.fire(this.getColorTheme());
		};
		this.media.addEventListener('change', listener);
		this.contrast.addEventListener('change', listener);
		this._register(
			toDisposable(() => {
				this.media.removeEventListener('change', listener);
				this.contrast.removeEventListener('change', listener);
			}),
		);
		this._register(nativeHostService.onDidChangeWindowState(state => {
			this.setSystemAccent(state.accentColor);
		}));
		nativeHostService.getWindowState().then(state => {
			this.setSystemAccent(state.accentColor);
		}, onUnexpectedError);
	}

	private setSystemAccent(color: string | undefined): void {
		if (color !== this.systemAccent && !this._store.isDisposed) {
			this.systemAccent = color;
			this.applySystemAccent();
		}
	}

	/**
	 * The palette's accent is the system's when the user's choice reads as a
	 * focus ring and selection on this theme's pages (3:1 against the canvas),
	 * and the product's otherwise. Text on it is white, unless only black reads
	 * on it. The palette falls back by itself where these are unset.
	 */
	private applySystemAccent(): void {
		const root = document.documentElement;
		const accent = this.systemAccent && Color.fromHex(this.systemAccent);
		if (!accent || accent.getContrastRatio(Color.fromHex(this.nativeColor('--canvas'))) < 3) {
			root.style.removeProperty('--system-accent');
			root.style.removeProperty('--system-on-accent');
			return;
		}
		root.style.setProperty('--system-accent', accent.toString());
		root.style.setProperty('--system-on-accent', accent.getContrastRatio(Color.white) >= 3 ? '#fff' : '#000');
	}

	public getColorTheme(): ITitlebarTheme {
		return {
			background: this.nativeColor('--opaque-chrome'),
			foreground: this.nativeColor('--text'),
			inactiveForeground: this.nativeColor('--muted'),
		};
	}

	private nativeColor(role: string): string {
		// Resolve semantic/system colors in Chromium, then send an opaque hex
		// value across the native controls contract. Vibrancy affects CSS surfaces.
		this.paletteProbe.style.color = `var(${role})`;
		const color = window.getComputedStyle(this.paletteProbe).color;
		const components = /^rgba?\(\s*(\d+)[, ]+\s*(\d+)[, ]+\s*(\d+)/.exec(color);
		if (!components) {
			throw new Error(`Unresolved theme color: ${role}`);
		}
		return '#' + components.slice(1, 4).map(value => Number(value).toString(16).padStart(2, '0')).join('');
	}
}

registerSingleton(IThemeService, ThemeService, InstantiationType.Delayed);

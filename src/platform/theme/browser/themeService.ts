/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: semantic workbench colors and native titlebar presentation.
 *--------------------------------------------------------------------------------------------*/

import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { Emitter } from '../../../base/common/event.js';
import { IThemeService, type ITitlebarTheme } from '../common/themeService.js';
import { registerSingleton, InstantiationType } from '../../instantiation/common/extensions.js';

export class ThemeService extends Disposable implements IThemeService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<ITitlebarTheme>());
	public readonly onDidColorThemeChange = this.changed.event;
	private readonly media = window.matchMedia('(prefers-color-scheme: dark)');
	private readonly contrast = window.matchMedia('(forced-colors: active)');
	private readonly paletteProbe = document.createElement('span');

	constructor() {
		super();
		this.paletteProbe.hidden = true;
		this.paletteProbe.style.forcedColorAdjust = 'none';
		document.documentElement.append(this.paletteProbe);
		this._register(toDisposable(() => {
			this.paletteProbe.remove();
		}));
		const listener = (): void => {
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

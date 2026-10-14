/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
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

	constructor() {
		super();
		const listener = (): void => {
			this.changed.fire(this.getColorTheme());
		};
		this.media.addEventListener('change', listener);
		this._register(
			toDisposable(() => {
				this.media.removeEventListener('change', listener);
			}),
		);
	}

	public getColorTheme(): ITitlebarTheme {
		return this.media.matches
			? { background: '#2c2d31', foreground: '#e6e6e8', inactiveForeground: '#999ba2' }
			: { background: '#ececef', foreground: '#242426', inactiveForeground: '#737378' };
	}
}

registerSingleton(IThemeService, ThemeService, InstantiationType.Delayed);

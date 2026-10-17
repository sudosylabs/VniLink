/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls, window state and view tabs.
 *--------------------------------------------------------------------------------------------*/

import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { Event } from '../../../base/common/event.js';
import { BrowserTitlebarPart } from './titlebarPart.js';
import { IThemeService } from '../../../platform/theme/common/themeService.js';
import { IHostService } from '../../services/host/host.js';
import { INativeHostService, type IWindowState } from '../../../platform/native/common/native.js';
import { getWorkbenchTitlebarHeight } from '../../../platform/window/common/titlebar.js';

export class NativeTitlebarPart extends BrowserTitlebarPart {
	private state: IWindowState | undefined;
	private controls = '';
	private readonly height: number;

	constructor(
		osRelease: string,
		onError: (error: unknown) => void,
		@IThemeService themeService: IThemeService,
		@IHostService hostService: IHostService,
		@INativeHostService private readonly nativeHostService: INativeHostService,
		@IInstantiationService instantiationService: IInstantiationService,
	) {
		super(onError, themeService, hostService, instantiationService);
		this.height = getWorkbenchTitlebarHeight(osRelease);
		this._register(
			nativeHostService.onDidChangeWindowState((state) => {
				this.acceptState(state);
			}),
		);
		this._register(Event.fromDOMEventEmitter<unknown>(window, 'resize')(() => {
			void this.initialize().catch(this.onError);
		}));
	}

	public async initialize(): Promise<void> {
		this.acceptState(await this.nativeHostService.getWindowState());
	}

	private acceptState(state: IWindowState): void {
		if (this._store.isDisposed || (this.state && state.revision <= this.state.revision)) {
			return;
		}
		this.state = state;
		this.active = state.focused;
		this.updateStyles();
	}

	protected override updateStyles(): void {
		super.updateStyles();
		if (!this.container || !this.state) {
			return;
		}
		document.documentElement.dataset.windowMaterial = this.state.backgroundMaterial;
		const height = this.height / this.state.zoomFactor;
		document.documentElement.style.setProperty('--window-zoom', String(this.state.zoomFactor));
		this.container.hidden = this.state.fullscreen;
		document.documentElement.style.setProperty('--titlebar-height', this.state.fullscreen ? '0px' : `${String(height)}px`);
		this.container.style.height = String(height) + 'px';
		const theme = this.themeService.getColorTheme();
		this.container.style.backgroundColor = this.state.backgroundMaterial === 'none' ? theme.background
			: this.state.backgroundMaterial === 'vibrancy' ? 'var(--native-chrome)' : 'transparent';
		const foreground = this.active ? theme.foreground : theme.inactiveForeground;
		const key = `${String(this.height)}:${theme.background}:${foreground}:${this.state.backgroundMaterial}`;
		if (this.controls !== key) {
			this.controls = key;
			void this.nativeHostService
				.updateWindowControls({
					height: this.height,
					backgroundColor: theme.background,
					foregroundColor: foreground,
				})
				.catch((error: unknown) => {
					this.controls = '';
					this.onError(error);
				});
		}
	}
}

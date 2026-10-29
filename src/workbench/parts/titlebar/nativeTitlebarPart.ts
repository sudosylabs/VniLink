/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls, window state and view tabs.
 *--------------------------------------------------------------------------------------------*/

import { addDisposableListener, EventType } from '../../../base/browser/dom.js';
import { onUnexpectedError } from '../../../base/common/errors.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { INativeHostService, type IWindowState } from '../../../platform/native/common/native.js';
import { IThemeService } from '../../../platform/theme/common/themeService.js';
import { getWorkbenchTitlebarHeight } from '../../../platform/window/common/titlebar.js';
import { IHostService } from '../../services/host/host.js';
import { BrowserTitlebarPart } from './titlebarPart.js';

export class NativeTitlebarPart extends BrowserTitlebarPart {
	private state: IWindowState | undefined;
	private controls = '';
	private readonly height: number;

	constructor(
		osRelease: string,
		@IThemeService themeService: IThemeService,
		@IHostService hostService: IHostService,
		@INativeHostService private readonly nativeHostService: INativeHostService,
		@IInstantiationService instantiationService: IInstantiationService,
	) {
		super(themeService, hostService, instantiationService);
		this.height = getWorkbenchTitlebarHeight(osRelease);
		this._register(nativeHostService.onDidChangeWindowState(state => {
			this.acceptState(state);
		}));
		this._register(addDisposableListener(window, EventType.RESIZE, () => {
			this.restore().catch(onUnexpectedError);
		}));
	}

	/** Reads the window state the titlebar lays out from. */
	public async restore(): Promise<void> {
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
		const container = this.getContainer();
		if (!container || !this.state) {
			return;
		}
		const root = document.documentElement;
		root.dataset.windowMaterial = this.state.backgroundMaterial;
		const height = this.height / this.state.zoomFactor;
		root.style.setProperty('--window-zoom', String(this.state.zoomFactor));
		container.hidden = this.state.fullscreen;
		root.style.setProperty('--titlebar-height', this.state.fullscreen ? '0px' : `${String(height)}px`);
		container.style.height = `${String(height)}px`;
		const theme = this.themeService.getColorTheme();
		container.style.backgroundColor = this.state.backgroundMaterial === 'none' ? theme.background
			: this.state.backgroundMaterial === 'vibrancy' ? 'var(--native-chrome)' : 'transparent';
		this.updateWindowControls(theme.background, this.active ? theme.foreground : theme.inactiveForeground);
	}

	/** Tells the native window controls the titlebar's height and colors when they change. */
	private updateWindowControls(background: string, foreground: string): void {
		const key = `${String(this.height)}:${background}:${foreground}:${this.state?.backgroundMaterial ?? ''}`;
		if (this.controls === key) {
			return;
		}
		this.controls = key;
		const controls = { height: this.height, backgroundColor: background, foregroundColor: foreground };
		this.nativeHostService.updateWindowControls(controls).catch((error: unknown) => {
			this.controls = '';
			onUnexpectedError(error);
		});
	}
}

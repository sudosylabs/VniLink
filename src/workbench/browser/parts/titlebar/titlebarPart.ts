/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import './media/titlebarpart.css';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { IThemeService } from '../../../../platform/theme/common/themeService.js';
import { IHostService } from '../../../services/host/browser/host.js';
export class BrowserTitlebarPart extends Disposable {
	protected container: HTMLElement | undefined;
	protected active = false;
	constructor(
		@IThemeService protected readonly themeService: IThemeService,
		@IHostService hostService: IHostService,
	) {
		super();
		this.active = hostService.hasFocus;
		this._register(
			hostService.onDidChangeFocus((active) => {
				this.active = active;
				this.updateStyles();
			}),
		);
		this._register(
			themeService.onDidColorThemeChange(() => {
				this.updateStyles();
			}),
		);
	}
	public create(parent: HTMLElement): void {
		this.container = parent;
		parent.className = 'part titlebar';
		const root = document.createElement('div');
		root.className = 'titlebar-container';
		const dragRegion = document.createElement('div');
		dragRegion.className = 'titlebar-drag-region';
		root.append(dragRegion);
		parent.append(root);
		this.updateStyles();
	}
	protected updateStyles(): void {
		if (!this.container) {
			return;
		}
		const theme = this.themeService.getColorTheme();
		this.container.style.backgroundColor = theme.background;
		this.container.style.color = this.active ? theme.foreground : theme.inactiveForeground;
		this.container.classList.toggle('inactive', !this.active);
	}
}

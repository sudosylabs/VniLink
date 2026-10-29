/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls, window state and view tabs.
 *--------------------------------------------------------------------------------------------*/

import './media/titlebarpart.css';
import './media/modernTabs.css';
import { $, append } from '../../../base/browser/dom.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { IThemeService } from '../../../platform/theme/common/themeService.js';
import { Part } from '../../part.js';
import { IHostService } from '../../services/host/host.js';
import { TitlebarTabsControl } from './titlebarTabsControl.js';

export class BrowserTitlebarPart extends Part {
	protected active: boolean;

	constructor(
		@IThemeService themeService: IThemeService,
		@IHostService hostService: IHostService,
		@IInstantiationService private readonly instantiationService: IInstantiationService,
	) {
		super('workbench.parts.titlebar', themeService);
		this.active = hostService.hasFocus;
		this._register(hostService.onDidChangeFocus(active => {
			this.active = active;
			this.updateStyles();
		}));
	}

	protected override createContentArea(parent: HTMLElement): HTMLElement {
		parent.className = 'part titlebar';
		const root = append(parent, $('.titlebar-container'));
		append(root, $('.titlebar-drag-region'));
		this._register(this.instantiationService.createInstance(TitlebarTabsControl, root));
		return root;
	}

	protected override updateStyles(): void {
		const container = this.getContainer();
		if (!container) {
			return;
		}
		const theme = this.themeService.getColorTheme();
		container.style.backgroundColor = theme.background;
		container.style.color = this.active ? theme.foreground : theme.inactiveForeground;
		container.classList.toggle('inactive', !this.active);
	}
}

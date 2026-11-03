// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../nls.js';
import { $, addDisposableListener, append, EventType, getClientArea, type IDimension } from '../base/browser/dom.js';
import { mainWindow } from '../base/browser/window.js';
import { setARIAContainer } from '../base/browser/ui/aria/aria.js';
import { Emitter, Event } from '../base/common/event.js';
import { Disposable } from '../base/common/lifecycle.js';
import type { ILayoutService } from '../platform/layout/browser/layoutService.js';
import type { Part } from './part.js';

/**
 * Arranges the workbench's parts in the window: the titlebar above a workspace that holds the notice line and the
 * views part. The workbench decides what the parts are. It is the window's
 * `ILayoutService`, whose container hosts context views and menus.
 */
export abstract class Layout extends Disposable implements ILayoutService {
	declare public readonly _serviceBrand: undefined;
	private readonly _onDidLayoutMainContainer = this._register(new Emitter<IDimension>());
	public readonly onDidLayoutMainContainer = this._onDidLayoutMainContainer.event;
	public readonly onDidLayoutContainer = Event.map(this.onDidLayoutMainContainer, dimension => ({ container: this.mainContainer, dimension }));
	private _mainContainerDimension: IDimension;
	private readonly titlebarContainer: HTMLElement;
	private readonly workspace: HTMLElement;
	private readonly notice: HTMLElement;
	private readonly viewsContainer: HTMLElement;

	constructor(protected readonly parent: HTMLElement) {
		super();
		this.titlebarContainer = $('header');
		this.workspace = $('.workspace');
		this.notice = append(this.workspace, $('.workbench-notice', { role: 'alert' }));
		this.notice.hidden = true;
		this.viewsContainer = append(this.workspace, $('div'));
		this._mainContainerDimension = getClientArea(parent);
		this._register(addDisposableListener(mainWindow, EventType.RESIZE, () => {
			this.layout();
		}));
		this._register({
			dispose: () => {
				this.titlebarContainer.remove();
				this.workspace.remove();
			},
		});
	}

	public get mainContainer(): HTMLElement {
		return this.parent;
	}

	public get activeContainer(): HTMLElement {
		return this.parent;
	}

	public getContainer(): HTMLElement {
		return this.parent;
	}

	public focus(): void {
		this.viewsContainer.focus();
	}

	public layout(): void {
		this._mainContainerDimension = getClientArea(this.parent);
		this._onDidLayoutMainContainer.fire(this._mainContainerDimension);
	}

	protected createWorkbenchLayout(titlebar: Part, views: Part): void {
		titlebar.create(this.titlebarContainer);
		views.create(this.viewsContainer);
		setARIAContainer(this.workspace);
		this.parent.replaceChildren(this.titlebarContainer, this.workspace);
	}

	/** Shows a message above the views; `undefined` hides it. */
	protected setNotice(message: string | undefined): void {
		this.notice.hidden = message === undefined;
		this.notice.textContent = message ?? '';
	}

	protected static readonly engineRestartingNotice = nls.localize('workbench.engineRestarting', 'VniLink’s connection service stopped and is starting again.');
	protected static readonly engineUnavailableNotice = nls.localize('workbench.engineUnavailable', 'VniLink could not start its connection service. Restart the app to try again.');
	protected static readonly unexpectedErrorNotice = nls.localize('workbench.actionFailed', 'The action could not be completed. Try again.');
}

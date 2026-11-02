// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Separator, toAction } from '../../../base/common/actions.js';
import { Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { IMenuService } from '../../../platform/actions/common/actions.js';
import { IContextKeyService } from '../../../platform/contextkey/common/contextkey.js';
import { ContextMenuService } from '../../../platform/contextview/browser/contextMenuService.js';
import { IContextMenuService, IContextViewService } from '../../../platform/contextview/browser/contextView.js';
import { ContextViewService } from '../../../platform/contextview/browser/contextViewService.js';
import { SyncDescriptor } from '../../../platform/instantiation/common/descriptors.js';
import { InstantiationService } from '../../../platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../../platform/instantiation/common/serviceCollection.js';
import { ILayoutService } from '../../../platform/layout/browser/layoutService.js';

/**
 * The context menu service as the workbench registers it, over the window's
 * body. `show` opens a menu with Rename and a checked Show Hidden Files, the
 * first item focused; the actions it runs are kept in `runs` and each close,
 * with whether it was cancelled, in `hides`. Menus offered by id are not
 * exercised, so the menu and context key services are bare.
 */
export class ContextMenuFixture extends Disposable {
	public readonly runs: string[] = [];
	public readonly hides: boolean[] = [];
	private readonly contextMenu: IContextMenuService;

	constructor() {
		super();
		const container = document.body;
		const services = this._register(new InstantiationService(new ServiceCollection(
			[ILayoutService, {
				_serviceBrand: undefined,
				mainContainer: container,
				activeContainer: container,
				containers: [container],
				getContainer: () => container,
				onDidLayoutContainer: Event.None,
			}],
			[IMenuService, { _serviceBrand: undefined }],
			[IContextKeyService, { _serviceBrand: undefined }],
			[IContextViewService, new SyncDescriptor(ContextViewService)],
			[IContextMenuService, new SyncDescriptor(ContextMenuService)],
		), true));
		this.contextMenu = services.invokeFunction(accessor => accessor.get(IContextMenuService));
	}

	public show(): void {
		const run = (id: string) => () => {
			this.runs.push(id);
		};
		this.contextMenu.showContextMenu({
			getAnchor: () => ({ x: 40, y: 60 }),
			getActions: () => [
				toAction({ id: 'rename', label: 'Rename', run: run('rename') }),
				new Separator(),
				toAction({ id: 'hidden', label: 'Show Hidden Files', checked: true, run: run('hidden') }),
			],
			autoSelectFirstItem: true,
			onHide: didCancel => {
				this.hides.push(didCancel);
			},
		});
	}
}

/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink from vs/workbench/services/contextmenu/electron-browser/contextmenuService.ts:
 *  native menus on macOS and VniLink's themed menus elsewhere, as its dialogs are, rather than
 *  a `window.menuStyle` setting; no telemetry, notification or keybinding service.
 *--------------------------------------------------------------------------------------------*/

import { type IAction, Separator, SubmenuAction } from '../../../../base/common/actions.js';
import * as dom from '../../../../base/browser/dom.js';
import { type IContextMenuMenuDelegate, IContextMenuService, IContextViewService } from '../../../../platform/contextview/browser/contextView.js';
import { getZoomFactor } from '../../../../base/browser/browser.js';
import { unmnemonicLabel } from '../../../../base/common/labels.js';
import type { IContextMenuDelegate, IContextMenuEvent } from '../../../../base/browser/contextmenu.js';
import { createSingleCallFunction } from '../../../../base/common/functional.js';
import type { IContextMenuItem } from '../../../../base/parts/contextmenu/common/contextmenu.js';
import { popup } from '../../../../base/parts/contextmenu/electron-browser/contextmenu.js';
import { isMacintosh, isWindows } from '../../../../base/common/platform.js';
import { ContextMenuMenuDelegate, ContextMenuService as HTMLContextMenuService } from '../../../../platform/contextview/browser/contextMenuService.js';
import { InstantiationType, registerSingleton } from '../../../../platform/instantiation/common/extensions.js';
import { stripIcons } from '../../../../base/common/iconLabels.js';
import { coalesce } from '../../../../base/common/arrays.js';
import { type Event, Emitter } from '../../../../base/common/event.js';
import { AnchorAlignment, AnchorAxisAlignment, isAnchor } from '../../../../base/browser/ui/contextview/contextview.js';
import { IMenuService } from '../../../../platform/actions/common/actions.js';
import { IContextKeyService } from '../../../../platform/contextkey/common/contextkey.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { INativeHostService } from '../../../../platform/native/common/native.js';
import { ILogService } from '../../../../platform/log/common/log.js';
import { StandardMouseEvent } from '../../../../base/browser/mouseEvent.js';

export class ContextMenuService implements IContextMenuService {

	declare readonly _serviceBrand: undefined;

	private readonly impl: HTMLContextMenuService | NativeContextMenuService;

	get onDidShowContextMenu(): Event<void> { return this.impl.onDidShowContextMenu; }
	get onDidHideContextMenu(): Event<void> { return this.impl.onDidHideContextMenu; }

	constructor(
		@IContextViewService contextViewService: IContextViewService,
		@IMenuService menuService: IMenuService,
		@IContextKeyService contextKeyService: IContextKeyService,
		@INativeHostService nativeHostService: INativeHostService,
		@ILogService logService: ILogService,
	) {
		this.impl = isMacintosh
			? new NativeContextMenuService(popup, menuService, contextKeyService, nativeHostService, logService)
			: new HTMLContextMenuService(contextViewService, menuService, contextKeyService);
	}

	dispose(): void {
		this.impl.dispose();
	}

	showContextMenu(delegate: IContextMenuDelegate | IContextMenuMenuDelegate): void {
		this.impl.showContextMenu(delegate);
	}
}

export class NativeContextMenuService extends Disposable implements IContextMenuService {

	declare readonly _serviceBrand: undefined;

	private readonly _onDidShowContextMenu = this._store.add(new Emitter<void>());
	readonly onDidShowContextMenu = this._onDidShowContextMenu.event;

	private readonly _onDidHideContextMenu = this._store.add(new Emitter<void>());
	readonly onDidHideContextMenu = this._onDidHideContextMenu.event;

	constructor(
		private readonly menuPopup: typeof popup,
		private readonly menuService: IMenuService,
		private readonly contextKeyService: IContextKeyService,
		private readonly nativeHostService: INativeHostService,
		private readonly logService: ILogService,
	) {
		super();
	}

	showContextMenu(delegate: IContextMenuDelegate | IContextMenuMenuDelegate): void {

		delegate = ContextMenuMenuDelegate.transform(delegate, this.menuService, this.contextKeyService);

		const actions = delegate.getActions();
		if (actions.length) {
			const onHide = createSingleCallFunction(() => {
				delegate.onHide?.(false);

				dom.ModifierKeyEmitter.getInstance().resetKeyStatus();
				this._onDidHideContextMenu.fire();
			});

			const anchor = delegate.getAnchor();
			const targetWindow = dom.isHTMLElement(anchor) ? dom.getWindow(anchor) : anchor instanceof StandardMouseEvent ? dom.getWindow(anchor.browserEvent) : dom.getActiveWindow();
			const menu = this.createMenu(delegate, actions, onHide);

			let x: number | undefined;
			let y: number | undefined;

			let zoom = getZoomFactor(targetWindow);
			if (dom.isHTMLElement(anchor)) {
				const clientRect = anchor.getBoundingClientRect();
				const elementPosition = { left: clientRect.left, top: clientRect.top, width: clientRect.width, height: clientRect.height };

				// Determine if element is clipped by viewport; if so we'll use the bottom-right of the visible portion
				const win = dom.getWindow(anchor);
				const vw = win.innerWidth;
				const vh = win.innerHeight;
				const isClipped = clientRect.left < 0 || clientRect.top < 0 || clientRect.right > vw || clientRect.bottom > vh;

				// When drawing context menus, we adjust the pixel position for native menus using zoom level
				// In areas where zoom is applied to the element or its ancestors, we need to adjust accordingly
				zoom *= dom.getDomNodeZoomLevel(anchor);

				if (isClipped) {
					// Element is partially out of viewport: always place at bottom-right visible corner
					x = Math.min(Math.max(clientRect.right, 0), vw);
					y = Math.min(Math.max(clientRect.bottom, 0), vh);
				} else {
					// Position according to the axis alignment and the anchor alignment:
					// `HORIZONTAL` aligns at the top left or right of the anchor and
					//  `VERTICAL` aligns at the bottom left of the anchor.
					if (delegate.anchorAxisAlignment === AnchorAxisAlignment.HORIZONTAL) {
						if (delegate.anchorAlignment === AnchorAlignment.LEFT) {
							x = elementPosition.left;
							y = elementPosition.top;
						} else {
							x = elementPosition.left + elementPosition.width;
							y = elementPosition.top;
						}

						if (!isMacintosh) {
							const window = dom.getWindow(anchor);
							const availableHeightForMenu = window.screen.height - y;
							if (availableHeightForMenu < actions.length * (isWindows ? 45 : 32) /* guess of 1 menu item height */) {
								// If the menu opens to the top, align it to the bottom of the anchor.
								y += elementPosition.height;
							}
						}
					} else {
						if (delegate.anchorAlignment === AnchorAlignment.LEFT) {
							x = elementPosition.left;
							y = elementPosition.top + elementPosition.height;
						} else {
							x = elementPosition.left + elementPosition.width;
							y = elementPosition.top + elementPosition.height;
						}
					}
				}

				// Shift macOS menus by a few pixels below elements
				// to account for extra padding on top of native menu
				// https://github.com/microsoft/vscode/issues/84231
				if (isMacintosh) {
					y += 4 / zoom;
				}
			} else if (isAnchor(anchor)) {
				x = anchor.x;
				y = anchor.y;
			} else {
				// We leave x/y undefined in this case which will result in
				// Electron taking care of opening the menu at the cursor position.
			}

			if (typeof x === 'number') {
				x = Math.floor(x * zoom);
			}

			if (typeof y === 'number') {
				y = Math.floor(y * zoom);
			}

			this.menuPopup(menu, { x, y, positioningItem: delegate.autoSelectFirstItem ? 0 : undefined, }, () => { onHide(); });

			this._onDidShowContextMenu.fire();
		}
	}

	private createMenu(delegate: IContextMenuDelegate, entries: readonly IAction[], onHide: () => void, submenuIds = new Set<string>()): IContextMenuItem[] {
		return coalesce(entries.map(entry => this.createMenuItem(delegate, entry, onHide, submenuIds)));
	}

	private createMenuItem(delegate: IContextMenuDelegate, entry: IAction, onHide: () => void, submenuIds: Set<string>): IContextMenuItem | undefined {
		// Separator
		if (entry instanceof Separator) {
			return { type: 'separator' };
		}

		// Submenu
		if (entry instanceof SubmenuAction) {
			if (submenuIds.has(entry.id)) {
				this.logService.warn(`Found submenu cycle: ${entry.id}`);
				return undefined;
			}

			return {
				label: unmnemonicLabel(stripIcons(entry.label)).trim(),
				submenu: this.createMenu(delegate, entry.actions, onHide, new Set([...submenuIds, entry.id]))
			};
		}

		// Normal Menu Item
		let type: 'radio' | 'checkbox' | undefined = undefined;
		if (entry.checked) {
			if (typeof delegate.getCheckedActionsRepresentation === 'function') {
				type = delegate.getCheckedActionsRepresentation(entry);
			} else {
				type = 'checkbox';
			}
		}

		const item: IContextMenuItem = {
			label: unmnemonicLabel(stripIcons(entry.label)).trim(),
			checked: !!entry.checked,
			type,
			enabled: !!entry.enabled,
			click: event => {

				// To preserve pre-electron-2.x behaviour, we first trigger
				// the onHide callback and then the action.
				// Fixes https://github.com/microsoft/vscode/issues/45601
				onHide();

				// Run action which will close the menu
				void this.runAction(entry, delegate, event);
			}
		};

		const keybinding = delegate.getKeyBinding?.(entry);
		if (keybinding) {
			const electronAccelerator = keybinding.getElectronAccelerator();
			if (electronAccelerator) {
				item.accelerator = electronAccelerator;
			} else {
				const label = keybinding.getLabel();
				if (label) {
					item.label = `${item.label} [${label}]`;
				}
			}
		}

		return item;
	}

	private async runAction(actionToRun: IAction, delegate: IContextMenuDelegate, event: IContextMenuEvent): Promise<void> {
		const context = delegate.getActionsContext ? delegate.getActionsContext(event) : undefined;

		try {
			// Native menus can run actions in an inactive application; DOM focus alone cannot activate it.
			await this.nativeHostService.focusWindow();

			if (delegate.actionRunner) {
				await delegate.actionRunner.run(actionToRun, context);
			} else if (actionToRun.enabled) {
				await actionToRun.run(context);
			}
		} catch (error) {
			this.logService.error('A context menu action failed.', error);
		}
	}
}

registerSingleton(IContextMenuService, ContextMenuService, InstantiationType.Delayed);

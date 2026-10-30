// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, append, type EventLike } from '../../../base/browser/dom.js';
import { ActionBar } from '../../../base/browser/ui/actionbar/actionbar.js';
import { BaseActionViewItem, type IBaseActionViewItemOptions } from '../../../base/browser/ui/actionbar/actionViewItems.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import type { IAction } from '../../../base/common/actions.js';
import { isIconId } from '../../../base/common/icons.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { IMenuService, MenuId, MenuItemAction } from '../../../platform/actions/common/actions.js';
import { IContextKeyService } from '../../../platform/contextkey/common/contextkey.js';
import { ITabService } from '../../services/tabs/tabService.js';

/** Actions for the active tab, shown at the end of the tab strip. Items choose a tab with `activeTab`. */
export const ViewActionsMenu = new MenuId('ViewActions');

/** What a view action receives when run from the toolbar: the button, so it can anchor a popover. */
export interface ViewActionContext {
	readonly anchor: HTMLElement;
}

export function isViewActionContext(value: unknown): value is ViewActionContext {
	return typeof value === 'object' && value !== null && 'anchor' in value && value.anchor instanceof HTMLElement;
}

/**
 * Renders a menu item as VniLink's icon button. The button stays disabled
 * while its command runs, so a slow action cannot be started twice.
 */
class ViewActionViewItem extends BaseActionViewItem {
	private button: HTMLButtonElement | undefined;
	private running = false;

	constructor(action: MenuItemAction, options: IBaseActionViewItemOptions) {
		super(undefined, action, options);
	}

	public override render(container: HTMLElement): void {
		super.render(container);
		const action = this.action;
		const button = this.button = append(container, $<HTMLButtonElement>('button.view-action', { type: 'button', tabindex: '-1' }));
		button.dataset.actionId = action.id;
		button.title = action.label;
		button.setAttribute('aria-label', action.label);
		const icon = action instanceof MenuItemAction ? action.item.icon : undefined;
		if (ThemeIcon.isThemeIcon(icon) && isIconId(icon.id)) {
			button.append(renderIcon(icon.id));
		} else {
			button.textContent = action.label;
		}
		this.updateEnabled();
	}

	public override onClick(event: EventLike): void {
		if (this.running || !this.action.enabled) {
			return;
		}
		event.preventDefault();
		this.running = true;
		this.updateEnabled();
		const context: ViewActionContext | undefined = this.button && { anchor: this.button };
		void Promise.resolve(this.actionRunner.run(this.action, context)).finally(() => {
			this.running = false;
			this.updateEnabled();
		});
	}

	public override focus(): void {
		this.button?.focus();
	}

	public override blur(): void {
		this.button?.blur();
	}

	public override setFocusable(focusable: boolean): void {
		if (this.button) {
			this.button.tabIndex = focusable ? 0 : -1;
		}
	}

	protected override updateEnabled(): void {
		if (this.button) {
			this.button.disabled = this.running || !this.action.enabled;
		}
	}
}

/** The active tab's toolbar, filled from {@link ViewActionsMenu}. */
export class ViewActionsToolbar extends Disposable {
	private readonly actionBar: ActionBar;

	constructor(
		container: HTMLElement,
		@IMenuService menuService: IMenuService,
		@IContextKeyService contextKeyService: IContextKeyService,
		@ITabService private readonly tabService: ITabService,
	) {
		super();
		this.actionBar = this._register(new ActionBar(container, {
			actionViewItemProvider: (action, options) => action instanceof MenuItemAction ? new ViewActionViewItem(action, options) : undefined,
		}));
		const menu = this._register(menuService.createMenu(ViewActionsMenu, contextKeyService));
		const update = (): void => {
			this.actionBar.clear();
			// Forwarded so each action receives the `ViewActionContext` of the button that ran it.
			const actions = menu.getActions({ shouldForwardArgs: true }).flatMap(([, group]) => group satisfies IAction[]);
			this.actionBar.push(actions, { icon: true, label: false });
			this.updateAriaLabel();
		};
		this._register(menu.onDidChange(update));
		this._register(tabService.onDidChange(() => {
			this.updateAriaLabel();
		}));
		update();
	}

	private updateAriaLabel(): void {
		const active = this.tabService.activeTab;
		this.actionBar.setAriaLabel(active ? nls.localize('tabs.tabActions', '{0} actions', active.label) : nls.localize('tabs.viewActions', 'View actions'));
	}
}

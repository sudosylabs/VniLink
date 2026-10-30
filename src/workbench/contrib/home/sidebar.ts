// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Emitter } from '../../../base/common/event.js';
import type { IconId } from '../../../base/common/icons.js';
import { Disposable, DisposableStore } from '../../../base/common/lifecycle.js';

export interface SidebarItem {
	readonly id: string;
	readonly label: string;
	readonly icon: IconId;
}

export interface SidebarGroup {
	readonly label?: string;
	readonly items: readonly SidebarItem[];
}

/** Home's navigation. It only reports selection; Home owns the pages. */
export class Sidebar extends Disposable {
	public readonly element: HTMLElement;
	private readonly nav: HTMLElement;
	private readonly selected = this._register(new Emitter<string>());
	public readonly onDidSelect = this.selected.event;
	private readonly items = new Map<string, { readonly element: HTMLButtonElement; readonly icon: IconId }>();
	private readonly itemListeners = this._register(new DisposableStore());
	private active: string | undefined;

	constructor(label: string) {
		super();
		this.element = $('aside.sidebar');
		this.nav = append(this.element, $('nav', { 'aria-label': label }));
		this._register({
			dispose: () => {
				this.element.remove();
			},
		});
	}

	/** Replaces the navigation items, keeping the selection when its item remains. */
	public setGroups(groups: readonly SidebarGroup[]): void {
		this.itemListeners.clear();
		this.items.clear();
		this.nav.replaceChildren(...groups.map(group => this.renderGroup(group)));
		const active = this.active;
		this.active = undefined;
		if (active) {
			this.select(active);
		}
	}

	private renderGroup(group: SidebarGroup): HTMLElement {
		const section = $('.sidebar-section');
		if (group.label) {
			append(section, $('h2.sidebar-heading')).textContent = group.label;
		}
		for (const item of group.items) {
			const entry = append(section, $<HTMLButtonElement>('button.nav-item', { type: 'button' }));
			entry.dataset.sectionId = item.id;
			entry.append(renderIcon(item.icon), item.label);
			this.itemListeners.add(addDisposableListener(entry, EventType.CLICK, () => {
				this.select(item.id);
			}));
			this.items.set(item.id, { element: entry, icon: item.icon });
		}
		return section;
	}

	public select(id: string): void {
		if (!this.items.has(id) || this.active === id) {
			return;
		}
		this.active = id;
		for (const [key, { element, icon }] of this.items) {
			const selected = key === id;
			element.classList.toggle('selected', selected);
			element.querySelector('.vnilink-icon')?.replaceWith(renderIcon(icon, { variant: selected ? 'filled' : 'outlined' }));
			if (selected) {
				element.setAttribute('aria-current', 'page');
			} else {
				element.removeAttribute('aria-current');
			}
		}
		this.selected.fire(id);
	}
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { $, append } from '../../../base/browser/dom.js';
import { Emitter, type Event } from '../../../base/common/event.js';
import type { IconId } from '../../../base/common/icons.js';
import { toDisposable, type IDisposable } from '../../../base/common/lifecycle.js';
import type { SyncDescriptor } from '../../../platform/instantiation/common/descriptors.js';
import { Registry } from '../../../platform/registry/common/platform.js';

/** A page shown in Home beside its sidebar. Pages are created when first shown and live as long as Home. */
export interface IHomePage extends IDisposable {
	readonly element: HTMLElement;
	/**
	 * The page fills Home's content area and scrolls its own regions, such as
	 * a list beside a full-height inspector. Otherwise Home scrolls the page.
	 */
	readonly scrollsItself?: boolean;
}

/** Sidebar sections; pages appear under their group in group order. */
export const enum HomePageGroup {
	Devices = 'devices',
	Settings = 'settings',
}

export interface IHomePageDescriptor {
	readonly id: string;
	readonly group: HomePageGroup;
	/** Position within the group; lower comes first. */
	readonly order: number;
	readonly title: string;
	readonly icon: IconId;
	readonly ctorDescriptor: SyncDescriptor<IHomePage>;
}

export const Extensions = {
	HomePages: 'workbench.registry.homePages',
};

/** Pages contributed to Home, in the shape of VS Code's views registry. */
export interface IHomePagesRegistry {
	readonly onDidChangePages: Event<void>;
	registerPage(page: IHomePageDescriptor): IDisposable;
	getPages(group: HomePageGroup): readonly IHomePageDescriptor[];
	getPage(id: string): IHomePageDescriptor | undefined;
}

class HomePagesRegistry implements IHomePagesRegistry {
	private readonly pages = new Map<string, IHomePageDescriptor>();
	private readonly pagesChanged = new Emitter<void>();
	public readonly onDidChangePages = this.pagesChanged.event;

	public registerPage(page: IHomePageDescriptor): IDisposable {
		if (this.pages.has(page.id)) {
			throw new Error(`Home page '${page.id}' is already registered`);
		}
		this.pages.set(page.id, page);
		this.pagesChanged.fire();
		return toDisposable(() => {
			if (this.pages.get(page.id) === page) {
				this.pages.delete(page.id);
				this.pagesChanged.fire();
			}
		});
	}

	public getPages(group: HomePageGroup): readonly IHomePageDescriptor[] {
		return [...this.pages.values()].filter(page => page.group === group).sort((left, right) => left.order - right.order);
	}

	public getPage(id: string): IHomePageDescriptor | undefined {
		return this.pages.get(id);
	}
}

Registry.add(Extensions.HomePages, new HomePagesRegistry());

/** The title and description every Home page starts with. */
export function createPageHeading(label: string, description: string): HTMLElement {
	const heading = $('header.page-heading');
	append(heading, $('h1')).textContent = label;
	append(heading, $('p')).textContent = description;
	return heading;
}

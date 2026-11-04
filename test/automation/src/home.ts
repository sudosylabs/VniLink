// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Code } from './code.js';

const SIDEBAR = '.sidebar';

/** The rounded panel inset in Home's page, which shows the window's material. */
const PANEL = `${SIDEBAR} > nav`;

const NAV_ITEM = `${SIDEBAR} .nav-item`;

const PAGE = '.home-content > main';

const DEVICE_ACTIONS = '.view-action[data-action-id^="vnilink.devices."]';

/** One section in the Home sidebar. */
export interface SidebarItem {
	readonly id: string;
	readonly label: string;
}

/** Home's panel: where it is, how round, and what the page paints in it. */
export interface HomePanel {
	readonly top: number;
	readonly left: number;
	readonly right: number;
	readonly bottom: number;
	readonly radius: string;
	readonly background: string;
}

/** Home: the sidebar and the page it shows. */
export class Home {

	constructor(private readonly code: Code) { }

	public async sidebarItems(): Promise<SidebarItem[]> {
		const items = await this.code.getElements(NAV_ITEM, false) ?? [];
		return items.map(item => ({ id: item.attributes['data-section-id'], label: item.textContent.trim() }));
	}

	public async sidebarHeading(): Promise<string | undefined> {
		return (await this.code.getElement(`${SIDEBAR} .sidebar-heading`))?.textContent;
	}

	/** Whether the retired branding or status bar is anywhere on the page. */
	public async hasRetiredChrome(): Promise<boolean> {
		return (await this.code.getElements('.status, .brand, footer', false) ?? []).length > 0;
	}

	public async open(section: string): Promise<void> {
		await this.code.waitAndClick(`${NAV_ITEM}[data-section-id="${section}"]`);
	}

	/** The sections marked as the page shown. */
	public async selected(): Promise<string[]> {
		return (await this.code.getElements(`${SIDEBAR} [aria-current="page"]`, false) ?? []).map(item => item.attributes['data-section-id']);
	}

	public async iconOf(section: string): Promise<{ readonly icon: string; readonly variant: string } | undefined> {
		const icon = await this.code.getElement(`${NAV_ITEM}[data-section-id="${section}"] .vnilink-icon`);
		return icon && { icon: icon.attributes['data-icon'], variant: icon.attributes['data-variant'] };
	}

	/** The variants of the icons of the sections not shown. */
	public async unselectedIconVariants(): Promise<string[]> {
		return (await this.code.getElements(`${NAV_ITEM}:not([aria-current]) .vnilink-icon`, false) ?? []).map(icon => icon.attributes['data-variant']);
	}

	/** Waits until the titlebar holds no Devices action, which menus drop after their debounce. */
	public async waitForNoDeviceActions(): Promise<void> {
		await this.code.waitForElements(DEVICE_ACTIONS, false, actions => actions.length === 0, 20);
	}

	public async deviceActionCount(): Promise<number> {
		return (await this.code.getElements(DEVICE_ACTIONS, false) ?? []).length;
	}

	/**
	 * The labels of the titlebar's actions once the menus have dropped those
	 * of the page shown before, after their debounce; what they are still,
	 * if any outlast it.
	 */
	public async titlebarActions(): Promise<string[]> {
		try {
			await this.code.waitForElements('.view-action', false, actions => actions.length === 0, 20);
		} catch {
			// Reported by the labels below.
		}
		return (await this.code.getElements('.view-action', false) ?? []).map(action => action.attributes['aria-label']);
	}

	/** Marks the page shown, so {@link showsMarkedPage} can say whether it is the same element later. */
	public markPage(name: string): Promise<void> {
		return this.code.driver.renderer.mark(PAGE, name);
	}

	public async showsMarkedPage(name: string): Promise<boolean> {
		return (await this.code.getElements(`${PAGE}[data-smoke-mark="${name}"]`, false) ?? []).length === 1;
	}

	/** Whether the Devices page itself offers pairing. */
	public async offersPairing(): Promise<boolean> {
		return (await this.code.getElements(`${PAGE} :is(.device-pairing-choice, .device-pairing-button)`, false) ?? []).length > 0;
	}

	public async sidebarBottom(): Promise<number> {
		return (await this.code.driver.renderer.getLayout(SIDEBAR)).bottom;
	}

	/** Where Home's panel is, how round, and what the page paints in it. */
	public async panel(): Promise<HomePanel> {
		const panel = await this.code.driver.renderer.getLayout(PANEL, ['border-top-left-radius', 'background-color']);
		return { top: panel.top, left: panel.left, right: panel.right, bottom: panel.bottom, radius: panel.style['border-top-left-radius'], background: panel.style['background-color'] };
	}

	/** Where the page shown beside the panel is. */
	public async page(): Promise<{ readonly left: number; readonly right: number; readonly bottom: number }> {
		const content = await this.code.driver.renderer.getLayout('.home-content');
		return { left: content.left, right: content.right, bottom: content.bottom };
	}
}

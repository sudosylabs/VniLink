// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Code } from './code.js';

const TITLEBAR = '.part.titlebar';

const SELECTED_TAB = '.tab[aria-selected="true"]';

const HOME_TAB = '.tabs-container [role="tab"]';

const REGION = '-webkit-app-region';

const BACKGROUND = 'background-color';

/** The titlebar's own presentation. */
export interface TitlebarPresentation {
	readonly height: number;
	readonly drag: string;
	readonly unwanted: number;
}

/** Where the selected tab sits in the titlebar and which parts drag the window. */
export interface TabGeometry {
	readonly height: number;
	readonly topGap: number;
	readonly bottomGap: number;
	readonly radius: string;
	readonly fill: string;
	readonly strip: string;
	readonly container: string;
	readonly tab: string;
	readonly close: boolean;
	readonly action: string | null;
}

/** What the window paints behind the titlebar, sidebar and content. */
export interface Surfaces {
	readonly titlebar: string;
	readonly sidebar: string;
	readonly content: string;
}

/** Home's tab: permanent, unclosable, and the sidebar inside its panel. */
export interface HomeTab {
	readonly label: string | undefined;
	readonly sidebarInPanel: boolean;
	readonly closable: boolean;
	readonly draggable: boolean;
}

/** The window's titlebar and its tabs. */
export class Titlebar {

	constructor(private readonly code: Code) { }

	public async presentation(): Promise<TitlebarPresentation> {
		const bar = await this.code.driver.renderer.getLayout(TITLEBAR);
		const drag = await this.code.driver.renderer.getLayout('.titlebar-drag-region', [REGION]);
		return {
			height: bar.height,
			drag: drag.style[REGION],
			unwanted: (await this.code.getElements('.window-controls, .titlebar-center, .menubar', false) ?? []).length,
		};
	}

	private async region(selector: string): Promise<string> {
		return (await this.code.driver.renderer.getLayout(selector, [REGION])).style[REGION];
	}

	public async selectedTab(): Promise<TabGeometry> {
		const renderer = this.code.driver.renderer;
		const bar = await renderer.getLayout(TITLEBAR);
		const tab = await renderer.getLayout(SELECTED_TAB);
		const fill = await renderer.getLayout(`${SELECTED_TAB} .tab-fill`, ['border-top-left-radius', BACKGROUND]);
		const hasAction = (await this.code.getElements('.view-action', false) ?? []).length > 0;
		return {
			height: tab.height,
			topGap: tab.top - bar.top,
			bottomGap: bar.bottom - tab.bottom,
			radius: fill.style['border-top-left-radius'],
			fill: fill.style[BACKGROUND],
			strip: await this.region('.tabs-scrollable'),
			container: await this.region('.tabs-container'),
			tab: await this.region('.tab'),
			close: (await this.code.getElements(`${SELECTED_TAB} .tab-close`, false) ?? []).length > 0,
			action: hasAction ? await this.region('.view-action') : null,
		};
	}

	public async surfaces(): Promise<Surfaces> {
		const background = async (selector: string) => (await this.code.driver.renderer.getLayout(selector, [BACKGROUND])).style[BACKGROUND];
		return { titlebar: await background(TITLEBAR), sidebar: await background('.sidebar'), content: await background('.home-content') };
	}

	/** How far the toolbar ends from the edge the native controls reserve. */
	public async toolbarGap(): Promise<number> {
		const container = await this.code.driver.renderer.getLayout('.titlebar-container', ['padding-right']);
		const toolbar = await this.code.driver.renderer.getLayout('.view-actions');
		return container.right - toolbar.right - parseFloat(container.style['padding-right']);
	}

	public async waitForHeight(height: number): Promise<void> {
		await this.code.waitForLayout(TITLEBAR, [], layout => Math.abs(layout.height - height) < 0.1);
	}

	public async tabHeight(): Promise<number> {
		return (await this.code.driver.renderer.getLayout('.tab')).height;
	}

	/** Waits until the titlebar is drawn in `background`. */
	public async waitForBackground(background: string): Promise<void> {
		await this.code.waitForLayout(TITLEBAR, [BACKGROUND], layout => layout.style[BACKGROUND] === background);
	}

	public async waitForHidden(hidden: boolean): Promise<void> {
		await this.code.waitForLayout(TITLEBAR, [], layout => layout.hidden === hidden);
	}

	public async homeTab(): Promise<HomeTab> {
		return {
			label: (await this.code.getElement(`${HOME_TAB} .tab-label`))?.textContent,
			sidebarInPanel: (await this.code.getElements('.view-host > [role="tabpanel"] .sidebar', false) ?? []).length === 1,
			closable: (await this.code.getElements(`${HOME_TAB} .tab-close`, false) ?? []).length > 0,
			draggable: (await this.code.driver.renderer.getLayout(HOME_TAB)).draggable,
		};
	}
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Code } from './code.js';

const CONTENT = '.home-content';

const PAGE = `${CONTENT} > main`;

const PICKER = '.theme-mode-picker';

/** What a settings page shows that the layout checks look at. */
export interface SettingsPageFacts {
	readonly heading: string;
	readonly note: string;
	readonly submits: boolean;
	readonly refreshes: boolean;
	readonly overflows: boolean;
	readonly legacyRows: number;
	readonly text: string;
}

/** The theme picker on General, at the window's size. */
export interface ThemePickerFacts {
	readonly choices: number;
	readonly selected: string | undefined;
	readonly overflows: boolean;
	readonly contained: boolean;
	readonly headingVisible: boolean;
	readonly inGeneral: boolean;
}

/** A settings page in Home. */
export class Settings {

	constructor(private readonly code: Code) { }

	public async open(category: string): Promise<void> {
		await this.code.waitAndClick(`[data-section-id="${category}"]`);
		await this.code.driver.renderer.whenRendered();
	}

	public async facts(): Promise<SettingsPageFacts> {
		const page = await this.code.waitForElement(PAGE);
		const content = await this.code.driver.renderer.getLayout(CONTENT);
		const buttons = await this.code.getElements(`${PAGE} button`, false) ?? [];
		return {
			heading: (await this.code.getElement(`${PAGE} h1`))?.textContent ?? '',
			note: (await this.code.getElement(`${PAGE} .settings-page-note`))?.textContent.trim() ?? '',
			submits: (await this.code.getElements(`${PAGE} button[type="submit"]`, false) ?? []).length > 0,
			refreshes: buttons.some(button => button.textContent === 'Refresh'),
			overflows: content.scrollWidth > content.clientWidth,
			legacyRows: (await this.code.getElements(`${PAGE} :is(.settings-control-row, .access-toggle-row, .access-section)`, false) ?? []).length,
			text: page.textContent,
		};
	}

	/** Waits until the page has read what it shows. */
	public async waitUntilSettled(): Promise<void> {
		await this.code.waitForElement(PAGE, page => page !== undefined && page.attributes['aria-busy'] !== 'true', 100);
	}

	public setTheme(theme: string): Promise<void> {
		return this.code.driver.renderer.updateSetting('appearance.theme', theme);
	}

	public async themePicker(): Promise<ThemePickerFacts> {
		const content = await this.code.driver.renderer.getLayout(CONTENT);
		const choices = await this.code.driver.renderer.getLayouts(`${PICKER} .theme-choice`);
		const heading = await this.code.driver.renderer.getLayout('.general-page h1');
		return {
			choices: choices.length,
			selected: (await this.code.getElement(`${PICKER} input:checked`))?.attributes.value,
			overflows: content.scrollWidth > content.clientWidth,
			contained: choices.every(choice => choice.left >= content.left && choice.right <= content.right),
			headingVisible: heading.top >= content.top,
			inGeneral: (await this.code.getElements(`.general-page [data-section="appearance"] ${PICKER}`, false) ?? []).length === 1,
		};
	}
}

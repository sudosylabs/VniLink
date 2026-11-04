// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Code } from './code.js';

const WELCOME = '.onboarding';

/** The welcome tab, as Home's General page opens it again. */
export class Onboarding {

	constructor(private readonly code: Code) { }

	/** Opens the welcome from the General page's "Set Up VniLink Again". */
	public async open(): Promise<void> {
		await this.code.waitAndClick('.view-action[aria-label="Set Up VniLink Again"]');
		await this.code.waitForElement(`${WELCOME}:not([hidden])`);
	}

	/**
	 * How wide the welcome is, and the tab area it is shown in, once the tab
	 * fills a window whose content is `contentWidth` wide. The welcome is read
	 * after the tab, so both come from the same layout.
	 */
	public async widths(contentWidth: number): Promise<{ readonly welcome: number; readonly tab: number }> {
		// Under display scaling the window's size is rounded to whole device pixels.
		const tab = await this.code.waitForLayout('.view-host', [], layout => Math.abs(layout.width - contentWidth) < 1);
		const welcome = await this.code.waitForLayout(WELCOME, [], layout => layout.width > 0);
		return { welcome: welcome.width, tab: tab.width };
	}

	/** How wide the step shown is. */
	public async stepWidth(): Promise<number> {
		return (await this.code.waitForLayout(`${WELCOME} .onboarding-card`, [], layout => layout.width > 0)).width;
	}

	/**
	 * Walks from the welcome's first step to its permissions step as someone
	 * sharing this computer's screen, camera and microphone, and says whether
	 * the system asks for any permission there; where it asks for none, the
	 * welcome has no such step.
	 */
	public async openPermissions(): Promise<boolean> {
		await this.continue();
		await this.continue();
		await this.code.waitAndClick('#onboarding-role-reachable');
		await this.continue();
		for (const shared of ['screen', 'camera', 'microphone']) {
			const toggle = `#onboarding-sharing-${shared}`;
			if (!await this.code.getElement(`${toggle}:checked`)) {
				await this.code.waitAndClick(toggle);
				await this.code.waitForElement(`${toggle}:checked`);
			}
		}
		await this.continue();
		return !!await this.code.getElement(`${WELCOME} .onboarding-permission`);
	}

	/** How wide each permission row's text and button are, in the order shown. */
	public async permissionRows(): Promise<readonly { readonly row: number; readonly text: number; readonly button: number }[]> {
		const count = (await this.code.getElements(`${WELCOME} .onboarding-permission`, false))?.length ?? 0;
		const rows = [];
		for (let index = 1; index <= count; index++) {
			const row = `${WELCOME} .onboarding-permission:nth-child(${String(index)})`;
			// The row is laid out once it has a width; its parts may be squeezed to none.
			const width = async (selector: string): Promise<number> => (await this.code.waitForLayout(selector, [], () => true)).width;
			const shown = (await this.code.waitForLayout(row, [], layout => layout.width > 0)).width;
			const button = await this.code.getElement(`${row} .monaco-button`);
			rows.push({ row: shown, text: await width(`${row} .onboarding-choice-text`), button: button ? await width(`${row} .monaco-button`) : 0 });
		}
		return rows;
	}

	/** Moves to the next step and waits for it to show. */
	private async continue(): Promise<void> {
		const progress = `${WELCOME} .onboarding-progress`;
		const before = await this.code.waitForTextContent(progress);
		await this.code.waitAndClick(`${WELCOME} .onboarding-steps .monaco-button:last-child`);
		await this.code.waitForTextContent(progress, undefined, text => text !== before);
	}

	/** Leaves the welcome without changing anything. */
	public async skip(): Promise<void> {
		await this.code.waitAndClick(`${WELCOME} .onboarding-skip .monaco-button`);
		await this.code.waitForElements(WELCOME, false, elements => elements.length === 0);
	}
}

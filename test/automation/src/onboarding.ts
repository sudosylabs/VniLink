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

	/** How wide the welcome is, and the tab area it is shown in. */
	public async widths(): Promise<{ readonly welcome: number; readonly tab: number }> {
		const welcome = await this.code.waitForLayout(WELCOME, [], layout => layout.width > 0);
		const tab = await this.code.waitForLayout('.view-host', [], layout => layout.width > 0);
		return { welcome: welcome.width, tab: tab.width };
	}

	/** Leaves the welcome without changing anything. */
	public async skip(): Promise<void> {
		await this.code.waitAndClick(`${WELCOME} .onboarding-skip .monaco-button`);
		await this.code.waitForElements(WELCOME, false, elements => elements.length === 0);
	}
}

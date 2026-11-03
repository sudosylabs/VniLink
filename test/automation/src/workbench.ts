// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IRendererExposure } from '../../../src/workbench/services/driver/windowDriver.js';
import type { Code } from './code.js';

/** The workbench window as a whole: what its page may reach, and the page it opens on. */
export class Workbench {

	constructor(private readonly code: Code) { }

	public exposure(): Promise<IRendererExposure> {
		return this.code.driver.renderer.rendererExposure();
	}

	/** The heading of the page shown. */
	public async heading(): Promise<string | undefined> {
		return (await this.code.getElement('h1'))?.textContent;
	}

	/** Whether the workbench's error notice is hidden. */
	public async noticeHidden(): Promise<boolean> {
		const notice = await this.code.getElement('.workbench-notice');
		return notice !== undefined && 'hidden' in notice.attributes;
	}

	/** The native material the window draws behind its page: `none`, `vibrancy`, `glass` or `mica`. */
	public async material(): Promise<string | undefined> {
		return (await this.code.getElement('html'))?.attributes['data-window-material'];
	}

	/** Waits until the home page's content is drawn in `background`, with the window's material unchanged. */
	public async waitForContentBackground(background: string): Promise<void> {
		await this.code.waitForLayout('.home-content', ['background-color'], layout => layout.style['background-color'] === background);
	}

	/** The height of the window's page, as the window's own size gives it. */
	public async viewportHeight(): Promise<number> {
		return (await this.code.driver.renderer.getLayout('html')).clientHeight;
	}
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { BrowserWindow } from 'electron';
import { writeFileSync } from 'node:fs';

/** What the notice paints behind itself, over the window's material. */
export interface NoticeBackdrop {
	readonly material: string;
	readonly root: string;
	readonly body: string;
}

/**
 * The system action notice: its own small window, which has no workbench
 * and so no driver. Each read is one query of its page by selector.
 */
export class Notice {

	constructor(public readonly window: BrowserWindow) { }

	private text(selector: string): Promise<unknown> {
		return this.window.webContents.executeJavaScript(`document.querySelector(${JSON.stringify(selector)})?.textContent`);
	}

	private background(selector: string): Promise<unknown> {
		return this.window.webContents.executeJavaScript(`getComputedStyle(document.querySelector(${JSON.stringify(selector)})).backgroundColor`);
	}

	public async title(): Promise<string> {
		return String(await this.text('#notice-title'));
	}

	public async detail(): Promise<string> {
		return String(await this.text('#notice-detail'));
	}

	public async focused(): Promise<string> {
		return String(await this.window.webContents.executeJavaScript('document.activeElement?.textContent'));
	}

	public async backdrop(): Promise<NoticeBackdrop> {
		const material: unknown = await this.window.webContents.executeJavaScript('document.documentElement.dataset.windowMaterial');
		return { material: String(material), root: String(await this.background('html')), body: String(await this.background('body')) };
	}

	/** Waits, at most `ms`, for the detail to read other than `previous`. */
	public async waitForDetailChange(previous: string, ms: number): Promise<string> {
		const deadline = Date.now() + ms;
		for (; ;) {
			const detail = await this.detail();
			if (detail !== previous) {
				return detail;
			}
			if (Date.now() > deadline) {
				throw new Error(`The notice did not count down from "${previous}"`);
			}
			await new Promise(resolve => setTimeout(resolve, 100));
		}
	}

	/** Presses the notice's Cancel button, the last of its two. */
	public async cancel(): Promise<void> {
		const cancel = '.notice-buttons .monaco-button:last-child';
		if (await this.text(cancel) !== 'Cancel') {
			throw new Error('The notice has no Cancel button');
		}
		await this.window.webContents.executeJavaScript(`document.querySelector(${JSON.stringify(cancel)}).click()`);
	}

	public async screenshot(path: string): Promise<void> {
		writeFileSync(path, (await this.window.webContents.capturePage()).toPNG());
	}
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Code } from './code.js';

/** The restart confirmation as Windows and Linux draw it. */
export class Dialogs {

	constructor(private readonly code: Code) { }

	public async show(): Promise<void> {
		await (await this.code.driver.fixtures()).showDialogFixture();
		await this.code.waitForElement('.monaco-dialog-box');
		await this.code.driver.renderer.whenRendered();
	}

	/** Answers the dialog with Escape and waits for it to close. */
	public async dismiss(): Promise<void> {
		await this.code.driver.renderer.sendKey('Escape', 27);
		await this.code.waitForElements('.monaco-dialog-box', false, boxes => boxes.length === 0, 30);
	}
}

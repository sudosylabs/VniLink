// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Code } from './code.js';

/** A context menu opened on the fixture. */
export class ContextMenu {

	constructor(private readonly code: Code) { }

	public async show(): Promise<void> {
		await (await this.code.driver.fixtures()).showContextMenuFixture();
		await this.code.waitForElement('.monaco-menu');
		await this.code.driver.renderer.whenRendered();
	}

	/** Cancels the menu with Escape and waits for it to close. */
	public async dismiss(): Promise<void> {
		await (await this.code.driver.fixtures()).closeContextMenuFixture();
		await this.code.waitForElements('.monaco-menu', false, menus => menus.length === 0, 30);
	}
}

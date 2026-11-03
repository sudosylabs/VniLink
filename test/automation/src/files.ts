// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Code } from './code.js';

/** The Files tab fixture, mid-task. */
export class Files {

	constructor(private readonly code: Code) { }

	public async show(width: number, height: number): Promise<void> {
		await (await this.code.driver.fixtures()).showFilesFixture(width, height);
		await this.code.driver.renderer.whenRendered();
	}

	public async clear(): Promise<void> {
		await (await this.code.driver.fixtures()).clearFilesFixture();
	}

	/** Whether the tab scrolls sideways. */
	public async overflows(): Promise<boolean> {
		const view = await this.code.driver.renderer.getLayout('.files-view');
		return view.scrollWidth > view.clientWidth;
	}
}

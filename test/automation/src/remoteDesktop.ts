// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { DesktopFixtureState } from '../../smoke/fixtures/remoteDesktop.js';
import type { Code } from './code.js';

/** A controlling remote desktop tab over a painted picture. */
export class RemoteDesktop {

	constructor(private readonly code: Code) { }

	/** Shows the tab in `state` once its controls have finished sliding in. */
	public async show(state: DesktopFixtureState): Promise<void> {
		await (await this.code.driver.fixtures()).showDesktopFixture(state);
		await this.code.driver.renderer.whenAnimationsFinished('body');
	}

	public async clear(): Promise<void> {
		await (await this.code.driver.fixtures()).clearDesktopFixture();
	}
}

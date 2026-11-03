// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { app } from 'electron';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { isWindows } from '../../base/common/platform.js';
import { isBuilt } from '../../platform/environment/electron-main/environment.js';
import { VniLinkApplication } from './app.js';
import { planLaunch } from './launch.js';

/** Exit code of a built application refusing a test or debugging switch. */
const REFUSED_SWITCH_EXIT_CODE = 2;

/** Establishes process identity and explicit test isolation before services read paths. */
export class VniLinkMain {
	public async main(): Promise<void> {
		const plan = planLaunch({
			argv: process.argv,
			built: isBuilt(),
			windows: isWindows,
			hasSwitch: name => app.commandLine.hasSwitch(name),
			testProfile: process.env.VNILINK_TEST_PROFILE,
		});
		app.setName(plan.name);
		if (plan.appUserModelId !== undefined) {
			app.setAppUserModelId(plan.appUserModelId);
		}
		const action = plan.action;
		switch (action.kind) {
			case 'refuse':
				console.error(`VniLink does not accept ${action.argument}.`);
				app.exit(REFUSED_SWITCH_EXIT_CODE);
				return;
			case 'invalid':
				throw new Error(action.reason);
			case 'selfCheck': {
				const profile = await mkdtemp(join(tmpdir(), 'vnilink-self-check-'));
				app.setPath('userData', profile);
				await new VniLinkApplication({ temporaryEngine: true, showWindow: false, selfCheck: { output: action.output, profile } }).start();
				return;
			}
			case 'start':
				if (action.userData !== undefined) {
					app.setPath('userData', action.userData);
				}
				await new VniLinkApplication(action.options).start();
				return;
		}
	}
}

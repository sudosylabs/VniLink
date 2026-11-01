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
import { product } from '../../platform/product/common/product.js';
import { VniLinkApplication } from './app.js';
import { selfCheckOutput } from './selfCheck.js';

/** Exit code of a built application refusing a test or debugging switch. */
const REFUSED_SWITCH_EXIT_CODE = 2;

/**
 * A built application exposes no automation or debugging surface: the smoke
 * driver and Chromium's remote debugging would let anyone who can start it
 * drive the workbench and its engine.
 */
function refusedSwitch(argv: readonly string[]): string | undefined {
	if (argv.includes('--smoke-test')) {
		return '--smoke-test';
	}
	return ['remote-debugging-port', 'remote-debugging-pipe'].find(name => app.commandLine.hasSwitch(name));
}

/** Establishes process identity and explicit test isolation before services read paths. */
export class VniLinkMain {
	public async main(): Promise<void> {
		app.setName(product.name);
		if (isWindows) {
			app.setAppUserModelId(product.appId);
		}
		const refused = isBuilt() ? refusedSwitch(process.argv) : undefined;
		if (refused) {
			console.error(`VniLink does not accept ${refused}.`);
			app.exit(REFUSED_SWITCH_EXIT_CODE);
			return;
		}
		const output = selfCheckOutput(process.argv);
		if (output !== undefined) {
			const profile = await mkdtemp(join(tmpdir(), 'vnilink-self-check-'));
			app.setPath('userData', profile);
			await new VniLinkApplication({ temporaryEngine: true, showWindow: false, selfCheck: { output, profile } }).start();
			return;
		}
		const smoke = process.argv.includes('--smoke-test');
		if (smoke) {
			const profile = process.env.VNILINK_TEST_PROFILE;
			if (!profile) {
				throw new Error('--smoke-test requires an isolated VNILINK_TEST_PROFILE');
			}
			app.setPath('userData', profile);
		}
		await new VniLinkApplication({ temporaryEngine: smoke, automationDriver: smoke }).start();
	}
}

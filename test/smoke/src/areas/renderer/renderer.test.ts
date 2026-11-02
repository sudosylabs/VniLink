// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import { app } from 'electron';
import Mocha from 'mocha';
import { isPermissionSnapshot } from '../../../../../src/platform/permissions/common/permissions.js';
import { isNativeWindowConfiguration } from '../../../../../src/platform/window/common/windowConfiguration.js';
import type { Automation } from '../../../../automation/src/automation.js';

/** The isolated renderer every run starts from, whichever scenarios it selects. */
export function setup(automation: Automation): void {
	Mocha.describe('renderer', () => {
		Mocha.it('reaches only the preload\'s API, and the main process only through prefixed channels', async () => {
			const exposure = await automation.workbench.exposure();
			assert.equal(exposure.nodeExposed, false, 'the page reaches Node');
			assert.equal(exposure.vscodeExposed, false, 'the page reaches a `vscode` global');
			assert.equal(exposure.unprefixedRejected, true, 'an unprefixed IPC channel answered');
			assert.deepEqual(exposure.api, ['context', 'ipcMessagePort', 'ipcRenderer', 'process', 'webUtils']);
			assert(isNativeWindowConfiguration(exposure.configuration), 'the window configuration is invalid');
			assert.equal(exposure.configuration.version, app.getVersion());
		});
		Mocha.it('cannot open another window, whatever script runs in it', async () => {
			assert.equal((await automation.workbench.exposure()).windowOpened, false, 'a page opened a window');
		});
		Mocha.it('opens on Devices with no error shown', async () => {
			assert.equal(await automation.workbench.heading(), 'Devices');
			assert.equal(await automation.workbench.noticeHidden(), true);
		});
		Mocha.it('reads the computer\'s live permissions', async () => {
			assert(isPermissionSnapshot(await automation.driver.renderer.permissionStatus()), 'invalid live OS permission snapshot');
		});
	});
}

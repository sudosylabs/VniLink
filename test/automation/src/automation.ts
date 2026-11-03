// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { BrowserWindow } from 'electron';
import type { TestApplication } from './application.js';
import { Code } from './code.js';
import { ContextMenu } from './contextMenu.js';
import { Devices } from './devices.js';
import { Dialogs } from './dialogs.js';
import type { WindowDriver } from './driver.js';
import { Files } from './files.js';
import { Home } from './home.js';
import { Onboarding } from './onboarding.js';
import { RemoteDesktop } from './remoteDesktop.js';
import { Settings } from './settings.js';
import { Titlebar } from './titlebar.js';
import { Workbench } from './workbench.js';

/** The running application as a smoke area reaches it: its window, and a page object per area. */
export class Automation {
	public readonly code: Code;
	public readonly workbench: Workbench;
	public readonly home: Home;
	public readonly onboarding: Onboarding;
	public readonly settings: Settings;
	public readonly titlebar: Titlebar;
	public readonly devices: Devices;
	public readonly files: Files;
	public readonly dialogs: Dialogs;
	public readonly contextMenu: ContextMenu;
	public readonly remoteDesktop: RemoteDesktop;

	constructor(public readonly window: BrowserWindow, public readonly driver: WindowDriver, public readonly application: TestApplication) {
		this.code = new Code(driver);
		this.workbench = new Workbench(this.code);
		this.home = new Home(this.code);
		this.onboarding = new Onboarding(this.code);
		this.settings = new Settings(this.code);
		this.titlebar = new Titlebar(this.code);
		this.devices = new Devices(this.code);
		this.files = new Files(this.code);
		this.dialogs = new Dialogs(this.code);
		this.contextMenu = new ContextMenu(this.code);
		this.remoteDesktop = new RemoteDesktop(this.code);
	}

	public screenshot(name: string): Promise<void> {
		return this.driver.screenshot(name);
	}
}

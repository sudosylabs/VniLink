// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { BrowserWindow } from 'electron';
import { join } from 'node:path';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { isLinux, isMacintosh } from '../../../base/common/platform.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';
import { WORKBENCH_URL } from '../../../base/parts/ipc/electron-main/ipcMain.js';
import { recoverRenderer } from './rendererRecovery.js';
import { defaultBrowserWindowOptions } from './windows.js';

export const IWindowsMainService = createDecorator<WindowsMainService>('windowsMainService');

/** Creates the application window and resolves windows by id for main-process services. */
export class WindowsMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly windowOpened = this._register(new Emitter<BrowserWindow>());
	public readonly onDidOpenWindow = this.windowOpened.event;
	private mainWindow: BrowserWindow | undefined;
	/** Whether the main window's page has loaded; until then it stays hidden. */
	private loaded = false;
	/** A request to show the window that arrived while its page loaded. */
	private showWhenLoaded = false;

	constructor(@ILogService private readonly logService: ILogService) {
		super();
	}

	public getMainWindow(): BrowserWindow | undefined {
		return this.mainWindow && !this.mainWindow.isDestroyed() ? this.mainWindow : undefined;
	}

	public getWindowById(windowId: number): BrowserWindow | undefined {
		const window = this.getMainWindow();
		return window?.id === windowId ? window : undefined;
	}

	public async openMainWindow(show: boolean): Promise<BrowserWindow> {
		const window = this.createWindow();
		this.mainWindow = window;
		this.loaded = false;
		this.showWhenLoaded = false;
		window.once('closed', () => {
			if (this.mainWindow === window) {
				this.mainWindow = undefined;
			}
		});
		this.windowOpened.fire(window);
		try {
			await window.loadURL(WORKBENCH_URL);
			if (this.mainWindow === window) {
				this.loaded = true;
			}
			if (show || this.takeShowRequest()) {
				window.show();
			}
			return window;
		} catch (error) {
			if (!window.isDestroyed()) {
				window.destroy();
			}
			throw error;
		}
	}

	/** Whether someone asked for the window while it loaded; asking once is answered once. */
	private takeShowRequest(): boolean {
		const requested = this.showWhenLoaded;
		this.showWhenLoaded = false;
		return requested;
	}

	public focusMainWindow(): void {
		const window = this.getMainWindow();
		if (window && !this.loaded) {
			// Shown before its page, the window would flash blank.
			this.showWhenLoaded = true;
			return;
		}
		if (window?.isMinimized()) {
			window.restore();
		}
		window?.show();
		window?.focus();
	}

	private createWindow(): BrowserWindow {
		const options = defaultBrowserWindowOptions();
		// Packaged builds and the development Electron (`build/lib/electron.ts`) both ship this resource.
		const icon = isLinux ? { icon: join(process.resourcesPath, 'app-icon.png') } : {};
		const window = new BrowserWindow({ ...options, ...icon, webPreferences: { ...options.webPreferences, preload: join(__dirname, 'preload.cjs') } });
		if (!isMacintosh) {
			window.removeMenu();
		}
		recoverRenderer(window, this.logService);
		return window;
	}
}

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
import { WORKBENCH_URL } from '../../../base/parts/ipc/electron-main/ipcMain.js';
import { defaultBrowserWindowOptions } from './windows.js';

export const IWindowsMainService = createDecorator<WindowsMainService>('windowsMainService');

/** Creates the application window and resolves windows by id for main-process services. */
export class WindowsMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly windowOpened = this._register(new Emitter<BrowserWindow>());
	public readonly onDidOpenWindow = this.windowOpened.event;
	private mainWindow: BrowserWindow | undefined;

	public getMainWindow(): BrowserWindow | undefined {
		return this.mainWindow && !this.mainWindow.isDestroyed() ? this.mainWindow : undefined;
	}

	public getWindowById(windowId: number): BrowserWindow | undefined {
		const window = this.getMainWindow();
		return window?.id === windowId ? window : undefined;
	}

	/** Whether the window currently shows the workbench, so it may receive privileged messages. */
	public isWorkbenchWindow(window: BrowserWindow): boolean {
		return !window.isDestroyed() && window.webContents.mainFrame.url === WORKBENCH_URL;
	}

	public async openMainWindow(show: boolean): Promise<BrowserWindow> {
		const window = this.createWindow();
		this.mainWindow = window;
		window.once('closed', () => {
			if (this.mainWindow === window) {
				this.mainWindow = undefined;
			}
		});
		this.windowOpened.fire(window);
		try {
			await window.loadURL(WORKBENCH_URL);
			if (show) {
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

	public focusMainWindow(): void {
		const window = this.getMainWindow();
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
		window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
		window.webContents.on('will-navigate', event => {
			event.preventDefault();
		});
		window.webContents.on('will-redirect', event => {
			event.preventDefault();
		});
		return window;
	}
}

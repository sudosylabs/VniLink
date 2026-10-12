import { release } from 'node:os';
import { isNativeHostRequest, type IWindowControlsOptions } from '../../native/common/native.js';
import type { NativeHostMainService } from '../../native/electron-main/nativeHostMainService.js';
import type { EngineService } from '../../engine/electron-main/engineService.js';
import { isEngineCommand } from '../../engine/common/engine.js';
import type { BrowserWindow } from 'electron';
import { app, ipcMain } from 'electron';
import type { AppInfo } from '../common/desktopApi.js';
export function registerDesktopHandlers(
	getWindow: () => BrowserWindow | null,
	entry: string,
	engine: EngineService,
	nativeHost: NativeHostMainService,
): void {
	function trusted(event: Electron.IpcMainInvokeEvent): BrowserWindow {
		const window = getWindow();
		if (
			event.sender !== window?.webContents ||
			event.senderFrame !== window.webContents.mainFrame ||
			event.senderFrame.url !== entry
		) {
			throw new Error('Untrusted desktop request');
		}
		return window;
	}
	ipcMain.handle('engine:command', async (event, command: unknown) => {
		trusted(event);
		if (!isEngineCommand(command)) {
			throw new Error('Invalid engine command');
		}
		return await engine.execute(command);
	});
	ipcMain.handle('app:info', (event): AppInfo => {
		trusted(event);
		const platform = process.platform;
		if (platform !== 'darwin' && platform !== 'win32' && platform !== 'linux') {
			throw new Error('Unsupported platform');
		}
		return {
			windowId: trusted(event).id,
			osRelease: release(),
			platform,
			version: app.getVersion(),
			engineConnected: engine.available,
		};
	});
	ipcMain.handle('nativeHost:command', async (event, method: unknown, args: unknown) => {
		const window = trusted(event);
		if (!isNativeHostRequest(method, args) || !Array.isArray(args) || args[0] !== window.id) {
			throw new Error('Invalid native host request');
		}
		switch (method) {
			case 'getWindowState':
				return await nativeHost.getWindowState(window.id);
			case 'focusWindow': {
				await nativeHost.focusWindow(window.id);
				return;
			}
			case 'minimizeWindow': {
				await nativeHost.minimizeWindow(window.id);
				return;
			}
			case 'maximizeWindow': {
				await nativeHost.maximizeWindow(window.id);
				return;
			}
			case 'unmaximizeWindow': {
				await nativeHost.unmaximizeWindow(window.id);
				return;
			}
			case 'closeWindow': {
				await nativeHost.closeWindow(window.id);
				return;
			}
			case 'setFullScreen': {
				await nativeHost.setFullScreen(window.id, args[1] as boolean);
				return;
			}
			case 'updateWindowControls': {
				await nativeHost.updateWindowControls(window.id, args[1] as IWindowControlsOptions);
				return;
			}
		}
	});
}

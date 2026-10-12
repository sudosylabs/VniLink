import type { EngineService } from '../../engine/electron-main/engineService.js';
import { isEngineCommand } from '../../engine/common/engine.js';
import type { BrowserWindow } from 'electron';
import { app, ipcMain } from 'electron';
import type { AppInfo, WindowAction } from '../common/desktopApi.js';
export function registerDesktopHandlers(getWindow: () => BrowserWindow | null, entry: string, engine: EngineService): void {
	function trusted(event: Electron.IpcMainInvokeEvent): BrowserWindow {
		const window = getWindow();
		if (event.sender !== window?.webContents || event.senderFrame !== window.webContents.mainFrame || event.senderFrame.url !== entry) {
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
		return { platform, version: app.getVersion(), engineConnected: engine.available };
	});
	ipcMain.handle('window:action', (event, action: WindowAction) => {
		const window = trusted(event);
		switch (action) {
			case 'minimize': window.minimize(); break;
			case 'maximize':
				if (window.isMaximized()) {
					window.unmaximize();
				} else {
					window.maximize();
				}
				break;
			case 'close': window.close(); break;
			default: throw new Error('Unknown window action');
		}
	});
}

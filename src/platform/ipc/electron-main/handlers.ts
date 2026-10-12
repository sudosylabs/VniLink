import { release } from 'node:os';
import { platform as safePlatform } from '../../../base/common/process.js';
import { isNativeHostRequest, type IWindowControlsOptions } from '../../native/common/native.js';
import type { NativeHostMainService } from '../../native/electron-main/nativeHostMainService.js';
import type { EngineService } from '../../engine/electron-main/engineService.js';
import type { EncryptionMainService } from '../../encryption/electron-main/encryptionMainService.js';
import { isEncryptionText } from '../../encryption/common/encryption.js';
import { isEngineCommand } from '../../engine/common/engine.js';
import type { BrowserWindow } from 'electron';
import { app, ipcMain } from 'electron';
import type { AppInfo } from '../common/desktopApi.js';

export function registerDesktopHandlers(
	getWindow: () => BrowserWindow | null,
	entry: string,
	engine: EngineService,
	nativeHost: NativeHostMainService,
	encryption: EncryptionMainService,
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
	ipcMain.handle('encryption:encrypt', async (event, value: unknown) => {
		trusted(event);
		if (!isEncryptionText(value)) {
			throw new Error('Invalid secret value');
		}
		return await encryption.encrypt(value);
	});
	ipcMain.handle('encryption:decrypt', async (event, value: unknown) => {
		trusted(event);
		if (!isEncryptionText(value)) {
			throw new Error('Invalid encrypted value');
		}
		return await encryption.decrypt(value);
	});
	ipcMain.handle('encryption:available', event => {
		trusted(event);
		return encryption.isEncryptionAvailable();
	});
	ipcMain.handle('encryption:provider', event => {
		trusted(event);
		return encryption.getKeyStorageProvider();
	});
	ipcMain.handle('app:info', (event): AppInfo => {
		trusted(event);
		const platform = safePlatform;
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

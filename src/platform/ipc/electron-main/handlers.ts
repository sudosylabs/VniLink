import { release } from 'node:os';
import { platform as safePlatform } from '../../../base/common/process.js';
import { isNativeHostRequest, type IWindowControlsOptions } from '../../native/common/native.js';
import type { NativeHostMainService } from '../../native/electron-main/nativeHostMainService.js';
import type { EngineService } from '../../engine/electron-main/engineService.js';
import type { EncryptionMainService } from '../../encryption/electron-main/encryptionMainService.js';
import { isEncryptionText } from '../../encryption/common/encryption.js';
import { isEngineCommand } from '../../engine/common/engine.js';
import type { BrowserWindow } from 'electron';
import { isChannelRequest } from '../../engine/common/dataChannel.js';
import { randomUUID } from 'node:crypto';
import { app, ipcMain, MessageChannelMain } from 'electron';
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
	const owners = new Map<number, string>();
	ipcMain.handle('engine:open-data-channel', (event, request: unknown) => {
		const window = trusted(event);
		if (!isChannelRequest(request)) {
			throw new Error('Invalid data channel request');
		}
		let owner = owners.get(window.id);
		if (!owner) {
			owner = randomUUID(); owners.set(window.id, owner);
			const channelOwner = owner;
			const navigate = (_event: Electron.Event, _url: string, _inPlace: boolean, mainFrame: boolean): void => {
				if (mainFrame) {
					release();
				}
			};
			const release = (): void => {
				if (owners.get(window.id) === channelOwner) {
					engine.closeChannels(channelOwner); owners.delete(window.id);
				}
				window.webContents.removeListener('destroyed', release);
				window.webContents.removeListener('render-process-gone', release);
				window.webContents.removeListener('did-start-navigation', navigate);
			};
			window.webContents.once('destroyed', release);
			window.webContents.once('render-process-gone', release);
			window.webContents.on('did-start-navigation', navigate);
		}
		const { port1, port2 } = new MessageChannelMain();
		try {
			engine.attachChannel(port1, request, owner); window.webContents.postMessage('engine:data-port', request.requestId, [port2]);
		} catch (error) {
			port1.close(); port2.close(); throw error;
		}
	});
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

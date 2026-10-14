import { synchronizeNativePermissions } from '../../engine/electron-main/nativeRuntime.js';
import { MachinePermissionService } from '../../permissions/electron-main/permissionService.js';
import { isPermissionCommand } from '../../permissions/common/permissions.js';
import { release } from 'node:os';
import { platform as safePlatform } from '../../../base/common/process.js';
import { isNativeHostRequest, type IWindowControlsOptions } from '../../native/common/native.js';
import type { NativeHostMainService } from '../../native/electron-main/nativeHostMainService.js';
import type { EngineService } from '../../engine/electron-main/engineService.js';
import type { EncryptionMainService } from '../../encryption/electron-main/encryptionMainService.js';
import { isEncryptionText } from '../../encryption/common/encryption.js';
import { isEngineCommand, isNativeScreenSource } from '../../engine/common/engine.js';
import type { BrowserWindow } from 'electron';
import { isChannelRequest } from '../../engine/common/dataChannel.js';
import { randomUUID } from 'node:crypto';
import { app, ipcMain, MessageChannelMain, systemPreferences, shell, screen } from 'electron';
import type { AppInfo } from '../common/desktopApi.js';
import type { ConfigurationStore } from '../../configuration/node/configurationStore.js';
import { isConfigurationUpdate } from '../../configuration/common/configuration.js';

export function registerDesktopHandlers(
	getWindow: () => BrowserWindow | null,
	entry: string,
	engine: EngineService,
	nativeHost: NativeHostMainService,
	encryption: EncryptionMainService,
	configuration: ConfigurationStore,
	requestRestart: () => void,
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
	const permissions = new MachinePermissionService(safePlatform, !app.isPackaged, systemPreferences, url => shell.openExternal(url));
	ipcMain.handle('permissions:command', async (event, command: unknown) => {
		trusted(event);
		if (!isPermissionCommand(command)) {
			throw new Error('Invalid permission command');
		}
		const snapshot = await permissions.execute(command);
		await synchronizeNativePermissions(engine, safePlatform, snapshot).catch(() => undefined);
		return snapshot;
	});
	let restartRevision = 0;
	async function restartState() {
		const reply = await engine.execute({ type: 'relayRestartRequired' });
		if (reply.type !== 'restartRequired') {
			throw new Error('Invalid restart state');
		}
		return { revision: ++restartRevision, required: reply.required };
	}
	ipcMain.handle('app:restart-state', async event => {
		trusted(event); return await restartState();
	});
	ipcMain.handle('app:restart', async event => {
		trusted(event); if ((await restartState()).required) {
			requestRestart();
		}
	});
	const captureDisplays = () => safePlatform === 'darwin' ? screen.getAllDisplays().slice(0, 16).map(display => ({ displayId: display.id, width: display.bounds.width, height: display.bounds.height, pixelWidth: Math.round(display.bounds.width * display.scaleFactor), pixelHeight: Math.round(display.bounds.height * display.scaleFactor) })).filter(isNativeScreenSource) : [];
	ipcMain.handle('app:capture-displays', event => {
		trusted(event); return captureDisplays();
	});
	ipcMain.handle('app:relay-documentation', async event => {
		trusted(event); await shell.openExternal('https://docs.iroh.computer/relays/rate-limiting');
	});
	const owners = new Map<number, string>();
	ipcMain.handle('configuration:get', event => {
		trusted(event);
		return configuration.snapshot;
	});
	let configurationPending: Promise<unknown> = Promise.resolve();
	ipcMain.handle('configuration:update', async (event, change: unknown) => {
		trusted(event);
		if (!isConfigurationUpdate(change)) {
			throw new Error('Invalid configuration update');
		}
		const result = configurationPending.then(async () => {
			const previous = configuration.snapshot;
			if (previous.error) {
				throw new Error(previous.error);
			}
			const runtime = async (snapshot: typeof previous): Promise<void> => {
				if (!engine.available) {
					return;
				}
				if (change.key === 'desktop.codec') {
					await engine.execute({ type: 'setDesktopCodecDefault', codec: snapshot.values['desktop.codec'] });
				}
				if (change.key === 'input.defaults') {
					await engine.execute({ type: 'setInputDefaults', ...snapshot.values['input.defaults'] });
				}
				if (change.key === 'files.roots') {
					const paths = snapshot.values['files.roots'];
					await engine.execute(paths.length ? { type: 'setFileRoots', paths: [...paths] } : { type: 'enableFilesystemAccess' });
				}
			};
			await runtime({ ...previous, values: { ...previous.values, [change.key]: change.value } });
			let snapshot: typeof previous;
			try {
				snapshot = await configuration.updateValue(change);
			} catch (error) {
				await runtime(previous); throw error;
			}
			if (change.key === 'desktop.quality' && engine.available) {
				await engine.execute({ type: 'setDesktopQualityDefaults', quality: snapshot.values['desktop.quality'] });
			}
			return snapshot;
		});
		configurationPending = result.catch(() => undefined);
		return result;
	});
	ipcMain.handle('engine:open-data-channel', (event, request: unknown) => {
		const window = trusted(event);
		if (!isChannelRequest(request)) {
			throw new Error('Invalid data channel request');
		}
		let owner = owners.get(window.id);
		if (!owner) {
			owner = randomUUID(); owners.set(window.id, owner);
			const channelOwner = owner;
			const contents = window.webContents;
			const windowId = window.id;
			const navigate = (_event: Electron.Event, _url: string, _inPlace: boolean, mainFrame: boolean): void => {
				if (mainFrame) {
					release();
				}
			};
			const release = (): void => {
				if (owners.get(windowId) === channelOwner) {
					engine.closeChannels(channelOwner); owners.delete(windowId);
				}
				// Native-backed listener removal is invalid after WebContents destruction.
				if (!contents.isDestroyed()) {
					contents.removeListener('destroyed', release);
					contents.removeListener('render-process-gone', release);
					contents.removeListener('did-start-navigation', navigate);
				}
			};
			contents.once('destroyed', release);
			contents.once('render-process-gone', release);
			contents.on('did-start-navigation', navigate);
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
		if (command.type === 'startNativeScreenSharing' && (safePlatform !== 'darwin' || !permissions.snapshot().permissions.some(permission => permission.kind === 'screen' && permission.status === 'granted'))) {
			throw new Error('Screen sharing requires a supported native backend and Screen Recording permission.');
		}
		if (command.type === 'startNativeScreenSharing') {
			const display = captureDisplays().find(display => display.displayId === command.displayId);
			if (display?.width !== command.width || display.height !== command.height || display.pixelWidth !== command.pixelWidth || display.pixelHeight !== command.pixelHeight) {
				throw new Error('Display geometry changed. Reload display selection before sharing.');
			}
			await synchronizeNativePermissions(engine, safePlatform, permissions.snapshot());
		}
		const reply = await engine.execute(command);
		if (reply.type === 'done' && ['saveCustomRelays', 'setRelayPublic', 'setRelayCustom'].includes(command.type)) {
			await restartState().then(state => {
				const window = getWindow(); if (window && !window.isDestroyed()) {
					window.webContents.send('app:restart-state', state);
				}
			}, () => { /* The committed command still returns Done; readback reports uncertainty. */ });
		}
		return reply;
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

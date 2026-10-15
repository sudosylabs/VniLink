/** Owns privileged IPC registration, request trust and window channel lifetimes. */
import { synchronizeNativePermissions } from '../../engine/electron-main/nativeRuntime.js';
import { MachinePermissionService } from '../../permissions/electron-main/permissionService.js';
import { isPermissionCommand } from '../../permissions/common/permissions.js';
import { release } from 'node:os';
import { join } from 'node:path';
import { mkdir } from 'node:fs/promises';
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
import { app, ipcMain, MessageChannelMain, systemPreferences, shell, screen, dialog } from 'electron';
import type { AppInfo } from '../common/desktopApi.js';
import type { ConfigurationStore } from '../../configuration/node/configurationStore.js';
import { isConfigurationUpdate } from '../../configuration/common/configuration.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';

export class DesktopIpcController extends Disposable {
	private permissions = new MachinePermissionService(safePlatform, !app.isPackaged, systemPreferences, url => shell.openExternal(url));
	private restartRevision = 0;
	private owners = new Map<number, string>();
	private configurationPending: Promise<unknown> = Promise.resolve();
	private synchronizedPermissions: string | undefined;
	private readonly releases = new Map<number, () => void>();

	constructor(private readonly getWindow: () => BrowserWindow | null,
		private readonly entry: string,
		private readonly engine: EngineService,
		private readonly nativeHost: NativeHostMainService,
		private readonly encryption: EncryptionMainService,
		private readonly configuration: ConfigurationStore,
		private readonly requestRestart: () => void) {
		super();
		try {
			this.registerPermissionsHandlers();
			this.registerApplicationHandlers();
			this.registerConfigurationHandlers();
			this.registerEngineHandlers();
			this.registerEncryptionHandlers();
			this.registerNativeHandlers();
		} catch (error) {
			this.dispose();
			throw error;
		}
	}

	private trusted(event: Electron.IpcMainInvokeEvent): BrowserWindow {
		if (this._store.isDisposed) {
			throw new Error('Desktop IPC disposed');
		}
		const window = this.getWindow();
		if (event.sender !== window?.webContents ||
			event.senderFrame !== window.webContents.mainFrame ||
			event.senderFrame.url !== this.entry) {
			throw new Error('Untrusted desktop request');
		}
		return window;
	}

	private async restartState() {
		const reply = await this.engine.execute({ type: 'relayRestartRequired' });
		if (reply.type !== 'restartRequired') {
			throw new Error('Invalid restart state');
		}
		return { revision: ++this.restartRevision, required: reply.required };
	}

	private captureDisplays() {
		return safePlatform === 'darwin' ? screen.getAllDisplays().slice(0, 16).map(display => ({ displayId: display.id, width: display.bounds.width, height: display.bounds.height, pixelWidth: Math.round(display.bounds.width * display.scaleFactor), pixelHeight: Math.round(display.bounds.height * display.scaleFactor) })).filter(isNativeScreenSource) : [];
	}

	private registerPermissionsHandlers(): void {
		this.handle('permissions:command', async (event, command: unknown) => {
			this.trusted(event);
			if (!isPermissionCommand(command)) {
				throw new Error('Invalid permission command');
			}
			const snapshot = await this.permissions.execute(command);
			const key = JSON.stringify(snapshot.permissions.map(permission => [permission.kind, permission.status]));
			if (this.engine.available && key !== this.synchronizedPermissions) {
				await synchronizeNativePermissions(this.engine, safePlatform, snapshot).then(() => {
					this.synchronizedPermissions = key;
				}).catch(() => { /* Keep OS state truthful; retry failed backend synchronization on the next read. */ });
			}
			return snapshot;
		});
	}

	private registerApplicationHandlers(): void {
		this.handle('app:recording-directory', async (event, action: unknown) => {
			const window = this.trusted(event);
			const directory = this.configuration.snapshot.values['recording.defaults'].directory || join(app.getPath('pictures'), 'VniLink');
			if (action === 'read') {
				return join(app.getPath('pictures'), 'VniLink');
			}
			if (action === 'choose') {
				const result = await dialog.showOpenDialog(window, { defaultPath: directory, properties: ['openDirectory', 'createDirectory'] });
				return result.canceled ? null : result.filePaths[0] ?? null;
			}
			if (action === 'destination') {
				if (this.configuration.snapshot.error) {
					throw new Error(this.configuration.snapshot.error);
				}
				await mkdir(directory, { recursive: true });
				return join(directory, `VniLink-${randomUUID()}.mkv`);
			}
			throw new Error('Invalid recording directory action');
		});
		this.handle('app:restart-state', async (event) => {
			this.trusted(event);
			return await this.restartState();
		});
		this.handle('app:restart', async (event) => {
			this.trusted(event);
			if ((await this.restartState()).required) {
				this.requestRestart();
			}
		});
		this.handle('app:capture-displays', event => {
			this.trusted(event);
			return this.captureDisplays();
		});
		this.handle('app:relay-documentation', async (event) => {
			this.trusted(event);
			await shell.openExternal('https://docs.iroh.computer/relays/rate-limiting');
		});
		this.handle('app:info', (event): AppInfo => {
			this.trusted(event);
			const platform = safePlatform;
			if (platform !== 'darwin' && platform !== 'win32' && platform !== 'linux') {
				throw new Error('Unsupported platform');
			}
			return {
				windowId: this.trusted(event).id,
				osRelease: release(),
				platform,
				version: app.getVersion(),
				engineConnected: this.engine.available,
			};
		});
	}

	private registerConfigurationHandlers(): void {
		this.handle('configuration:get', event => {
			this.trusted(event);
			return this.configuration.snapshot;
		});
		this.handle('configuration:update', async (event, change: unknown) => {
			this.trusted(event);
			if (!isConfigurationUpdate(change)) {
				throw new Error('Invalid configuration update');
			}
			const result = this.configurationPending.then(async () => {
				const previous = this.configuration.snapshot;
				if (previous.error) {
					throw new Error(previous.error);
				}
				const runtime = async (snapshot: typeof previous): Promise<void> => {
					if (!this.engine.available) {
						return;
					}
					if (change.key === 'desktop.quality') {
						await this.engine.execute({ type: 'setDesktopQualityDefaults', quality: snapshot.values['desktop.quality'] });
					}
					if (change.key === 'desktop.codec') {
						await this.engine.execute({ type: 'setDesktopCodecDefault', codec: snapshot.values['desktop.codec'] });
					}
					if (change.key === 'media.sources') {
						const sources = snapshot.values['media.sources'];
						await this.engine.execute({ type: 'setNativeCameraSources', ids: [...sources.cameras] });
						await this.engine.execute({ type: 'setNativeAudioSources', microphones: [...sources.microphones], system: [...sources.system] });
					}
					if (change.key === 'input.defaults') {
						await this.engine.execute({ type: 'setInputDefaults', ...snapshot.values['input.defaults'] });
					}
					if (change.key === 'files.roots') {
						const paths = snapshot.values['files.roots'];
						await this.engine.execute(paths.length ? { type: 'setFileRoots', paths: [...paths] } : { type: 'enableFilesystemAccess' });
					}
				};
				try {
					await runtime({ ...previous, values: { ...previous.values, [change.key]: change.value } });
				} catch (error) {
					await runtime(previous);
					throw error;
				}
				let snapshot: typeof previous;
				try {
					snapshot = await this.configuration.updateValue(change);
				} catch (error) {
					await runtime(previous);
					throw error;
				}
				if (change.key === 'desktop.quality' && this.engine.available) {
					await this.engine.execute({ type: 'setDesktopQualityDefaults', quality: snapshot.values['desktop.quality'] });
				}
				return snapshot;
			});
			this.configurationPending = result.catch(() => { /* The committed command remains valid when readback fails. */ });
			return result;
		});
	}

	private registerEngineHandlers(): void {
		this.handle('engine:open-data-channel', (event, request: unknown) => {
			const window = this.trusted(event);
			if (!isChannelRequest(request)) {
				throw new Error('Invalid data channel request');
			}
			let owner = this.owners.get(window.id);
			if (!owner) {
				owner = randomUUID();
				this.owners.set(window.id, owner);
				const channelOwner = owner;
				const contents = window.webContents;
				const windowId = window.id;
				const navigate = (_event: Electron.Event, _url: string, _inPlace: boolean, mainFrame: boolean): void => {
					if (mainFrame) {
						release();
					}
				};
				const release = (): void => {
					if (this.owners.get(windowId) === channelOwner) {
						this.engine.closeChannels(channelOwner);
						this.owners.delete(windowId);
						this.releases.delete(windowId);
					}
					// Native-backed listener removal is invalid after WebContents destruction.
					if (!contents.isDestroyed()) {
						contents.removeListener('destroyed', release);
						contents.removeListener('render-process-gone', release);
						contents.removeListener('did-start-navigation', navigate);
					}
				};
				this.releases.set(windowId, release);
				contents.once('destroyed', release);
				contents.once('render-process-gone', release);
				contents.on('did-start-navigation', navigate);
			}
			const { port1, port2 } = new MessageChannelMain();
			try {
				this.engine.attachChannel(port1, request, owner);
				window.webContents.postMessage('engine:data-port', request.requestId, [port2]);
			} catch (error) {
				port1.close();
				port2.close();
				throw error;
			}
		});
		this.handle('engine:command', async (event, command: unknown) => {
			this.trusted(event);
			if (!isEngineCommand(command)) {
				throw new Error('Invalid engine command');
			}
			if (command.type === 'startNativeScreenSharing' && (safePlatform !== 'darwin' || !this.permissions.snapshot().permissions.some(permission => permission.kind === 'screen' && permission.status === 'granted'))) {
				throw new Error('Screen sharing requires a supported native backend and Screen Recording permission.');
			}
			if (command.type === 'startNativeScreenSharing') {
				const display = this.captureDisplays().find(display => display.displayId === command.displayId);
				if (display?.width !== command.width || display.height !== command.height || display.pixelWidth !== command.pixelWidth || display.pixelHeight !== command.pixelHeight) {
					throw new Error('Display geometry changed. Reload display selection before sharing.');
				}
				await synchronizeNativePermissions(this.engine, safePlatform, this.permissions.snapshot());
			}
			const reply = await this.engine.execute(command);
			if (reply.type === 'done' && ['saveCustomRelays', 'setRelayPublic', 'setRelayCustom'].includes(command.type)) {
				await this.restartState().then(state => {
					const window = this.getWindow();
					if (window && !window.isDestroyed()) {
						window.webContents.send('app:restart-state', state);
					}
				}, () => { /* Committed command succeeds even if state readback fails. */ });
			}
			return reply;
		});
	}

	private registerEncryptionHandlers(): void {
		this.handle('encryption:encrypt', async (event, value: unknown) => {
			this.trusted(event);
			if (!isEncryptionText(value)) {
				throw new Error('Invalid secret value');
			}
			return await this.encryption.encrypt(value);
		});
		this.handle('encryption:decrypt', async (event, value: unknown) => {
			this.trusted(event);
			if (!isEncryptionText(value)) {
				throw new Error('Invalid encrypted value');
			}
			return await this.encryption.decrypt(value);
		});
		this.handle('encryption:available', event => {
			this.trusted(event);
			return this.encryption.isEncryptionAvailable();
		});
		this.handle('encryption:provider', event => {
			this.trusted(event);
			return this.encryption.getKeyStorageProvider();
		});
	}

	private registerNativeHandlers(): void {
		this.handle('nativeHost:command', async (event, method: unknown, args: unknown) => {
			const window = this.trusted(event);
			if (!isNativeHostRequest(method, args) || !Array.isArray(args) || args[0] !== window.id) {
				throw new Error('Invalid native host request');
			}
			switch (method) {
				case 'getWindowState':
					return await this.nativeHost.getWindowState(window.id);
				case 'focusWindow': {
					await this.nativeHost.focusWindow(window.id);
					return;
				}
				case 'minimizeWindow': {
					await this.nativeHost.minimizeWindow(window.id);
					return;
				}
				case 'maximizeWindow': {
					await this.nativeHost.maximizeWindow(window.id);
					return;
				}
				case 'unmaximizeWindow': {
					await this.nativeHost.unmaximizeWindow(window.id);
					return;
				}
				case 'closeWindow': {
					await this.nativeHost.closeWindow(window.id);
					return;
				}
				case 'setFullScreen': {
					await this.nativeHost.setFullScreen(window.id, args[1] as boolean);
					return;
				}
				case 'updateWindowControls': {
					await this.nativeHost.updateWindowControls(window.id, args[1] as IWindowControlsOptions);
					return;
				}
			}
		});
	}

	private handle(channel: string, listener: Parameters<typeof ipcMain.handle>[1]): void {
		ipcMain.handle(channel, listener);
		this._register(toDisposable(() => {
			ipcMain.removeHandler(channel);
		}));
	}

	public override dispose(): void {
		for (const release of this.releases.values()) {
			release();
		}
		this.releases.clear();
		super.dispose();
	}
}

export function registerDesktopHandlers(getWindow: () => BrowserWindow | null,
	entry: string,
	engine: EngineService,
	nativeHost: NativeHostMainService,
	encryption: EncryptionMainService,
	configuration: ConfigurationStore,
	requestRestart: () => void): DesktopIpcController {
	return new DesktopIpcController(getWindow, entry, engine, nativeHost, encryption, configuration, requestRestart);
}

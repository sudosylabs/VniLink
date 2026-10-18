import { app, BrowserWindow, nativeTheme, net, protocol, session } from 'electron';
import type { BrowserWindow as BrowserWindowType } from 'electron';
import { release } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setUnexpectedErrorHandler } from '../../base/common/errors.js';
import { Event } from '../../base/common/event.js';
import { Disposable, DisposableStore } from '../../base/common/lifecycle.js';
import { mark } from '../../base/common/performance.js';
import { platform } from '../../base/common/process.js';
import { ProxyChannel } from '../../base/parts/ipc/common/ipc.js';
import { Server as ElectronIPCServer } from '../../base/parts/ipc/electron-main/ipc.electron.js';
import { validatedIpcMain } from '../../base/parts/ipc/electron-main/ipcMain.js';
import { ConfigurationChannel } from '../../platform/configuration/electron-main/configurationChannel.js';
import { ConfigurationMainService, IConfigurationMainService } from '../../platform/configuration/electron-main/configurationMainService.js';
import { launchedAtLogin } from '../../platform/configuration/electron-main/systemSettings.js';
import { IEncryptionMainService } from '../../platform/encryption/common/encryption.js';
import { EncryptionMainService } from '../../platform/encryption/electron-main/encryptionMainService.js';
import { ENGINE_CHANNEL_NAME } from '../../platform/engine/common/engineService.js';
import { EngineChannel } from '../../platform/engine/electron-main/engineChannel.js';
import { EngineMainService, IEngineMainService } from '../../platform/engine/electron-main/engineMainService.js';
import { HostingMainService, IHostingMainService } from '../../platform/hosting/electron-main/hostingMainService.js';
import { ITrayMainService, TrayMainService } from '../../platform/tray/electron-main/trayMainService.js';
import { traySupported } from '../../platform/tray/electron-main/traySupport.js';
import { SyncDescriptor } from '../../platform/instantiation/common/descriptors.js';
import type { ServicesAccessor } from '../../platform/instantiation/common/instantiation.js';
import { InstantiationService } from '../../platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../platform/instantiation/common/serviceCollection.js';
import { ILifecycleMainService, LifecycleMainService } from '../../platform/lifecycle/electron-main/lifecycleMainService.js';
import { ConsoleMainLogger, ILogService, LogLevel } from '../../platform/log/common/log.js';
import { LogService } from '../../platform/log/common/logService.js';
import { installApplicationMenu, registerEditingShortcuts } from '../../platform/menubar/electron-main/menubar.js';
import { NATIVE_HOST_CHANNEL_NAME } from '../../platform/native/common/native.js';
import { NativeHostChannel } from '../../platform/native/electron-main/nativeHostChannel.js';
import { INativeHostMainService, NativeHostMainService } from '../../platform/native/electron-main/nativeHostMainService.js';
import { PERMISSIONS_CHANNEL_NAME } from '../../platform/permissions/common/permissions.js';
import { PermissionsChannel } from '../../platform/permissions/electron-main/permissionsChannel.js';
import { IPermissionsMainService, PermissionsMainService } from '../../platform/permissions/electron-main/permissionsMainService.js';
import { RECORDING_CHANNEL_NAME } from '../../platform/recording/common/recording.js';
import { IRecordingMainService, RecordingMainService } from '../../platform/recording/electron-main/recordingMainService.js';
import { CONFIGURATION_CHANNEL_NAME } from '../../platform/configuration/common/configurationSnapshot.js';
import { WINDOW_CONFIGURATION_CHANNEL, type INativeWindowConfiguration } from '../../platform/window/common/windowConfiguration.js';
import { IWindowsMainService, WindowsMainService } from '../../platform/windows/electron-main/windowsMainService.js';
import { registerDevelopmentWindow } from './development.js';

export interface ApplicationOptions {
	readonly showWindow?: boolean;
	readonly temporaryEngine?: boolean;
	readonly onReady?: (window: BrowserWindowType) => Promise<void>;
}

/** Files the `vnilink://app/` protocol serves; everything else is not found. */
const workbenchFiles = new Set(['/index.html', '/renderer.js', '/renderer.css', '/app-icon.svg']);

/**
 * Main-process composition: creates services, exposes them to windows over
 * IPC and opens the workbench window. Remote capability policy stays in Rust.
 */
export class VniLinkApplication extends Disposable {
	private readonly logService: LogService;
	private readonly instantiationService: InstantiationService;
	private started: Promise<void> | undefined;
	private restored = false;
	private opening: Promise<void> | undefined;

	constructor(private readonly options: ApplicationOptions = {}) {
		super();
		this.logService = this._register(new LogService(new ConsoleMainLogger(process.argv.includes('--verbose') ? LogLevel.Trace : LogLevel.Info)));
		setUnexpectedErrorHandler(error => {
			this.logService.error(error instanceof Error ? error : new Error(String(error)));
		});
		this.instantiationService = this._register(new InstantiationService(this.createServices(), true));
	}

	private createServices(): ServiceCollection {
		return new ServiceCollection(
			[ILogService, this.logService],
			[ILifecycleMainService, new SyncDescriptor(LifecycleMainService)],
			[IWindowsMainService, new SyncDescriptor(WindowsMainService)],
			[INativeHostMainService, new SyncDescriptor(NativeHostMainService)],
			[IEngineMainService, new SyncDescriptor(EngineMainService)],
			[IHostingMainService, new SyncDescriptor(HostingMainService)],
			[ITrayMainService, new SyncDescriptor(TrayMainService)],
			[IPermissionsMainService, new SyncDescriptor(PermissionsMainService)],
			[IConfigurationMainService, new SyncDescriptor(ConfigurationMainService, [join(app.getPath('userData'), 'settings.json')])],
			[IRecordingMainService, new SyncDescriptor(RecordingMainService)],
			[IEncryptionMainService, new SyncDescriptor(EncryptionMainService)],
		);
	}

	public start(): Promise<void> {
		this.started ??= this.startup();
		return this.started;
	}

	private async startup(): Promise<void> {
		mark('vnilink/mainStart');
		app.setName('VniLink');
		if (!app.requestSingleInstanceLock()) {
			this.dispose();
			app.quit();
			return;
		}
		protocol.registerSchemesAsPrivileged([{ scheme: 'vnilink', privileges: { standard: true, secure: true, supportFetchAPI: true } }]);
		this.instantiationService.invokeFunction(accessor => {
			this.registerListeners(accessor);
		});
		try {
			await app.whenReady();
			this.checkRunning();
			installApplicationMenu();
			await this.instantiationService.invokeFunction(accessor => accessor.get(IConfigurationMainService)).initialize();
			this.checkRunning();
			this.instantiationService.invokeFunction(accessor => {
				this.applyAppearance(accessor);
				this.configureSession();
				this.initChannels(accessor);
			});
			mark('vnilink/appReady');
			await this.startEngine();
			this.checkRunning();
			this.instantiationService.invokeFunction(accessor => {
				accessor.get(IHostingMainService).start();
			});
			if (await traySupported()) {
				this.checkRunning();
				this.instantiationService.invokeFunction(accessor => {
					this.createTray(accessor);
				});
			}
			if (!this.options.temporaryEngine && this.runsInBackground() && launchedAtLogin()) {
				// Started by the login item: accept paired devices without a window until the user opens one.
				this.restored = true;
				return;
			}
			const window = await this.openWindow();
			this.restored = true;
			mark('vnilink/windowReady');
			await this.options.onReady?.(window);
		} catch (error) {
			const lifecycle = this.instantiationService.invokeFunction(accessor => accessor.get(ILifecycleMainService));
			if (lifecycle.quitting || this._store.isDisposed) {
				return;
			}
			this.logService.error(error instanceof Error ? error : new Error(String(error)));
			await lifecycle.runShutdown();
			this.dispose();
			app.exit(1);
		}
	}

	private checkRunning(): void {
		const lifecycle = this.instantiationService.invokeFunction(accessor => accessor.get(ILifecycleMainService));
		if (lifecycle.quitting || this._store.isDisposed) {
			throw new Error('Application stopped during startup');
		}
	}

	private registerListeners(accessor: ServicesAccessor): void {
		const lifecycle = accessor.get(ILifecycleMainService);
		const windows = accessor.get(IWindowsMainService);
		const engine = accessor.get(IEngineMainService);
		const configuration = accessor.get(IConfigurationMainService);
		this._register(lifecycle.onWillShutdown(event => {
			event.join('engine', engine.stop());
			event.join('configuration', configuration.flush());
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'will-quit')(() => {
			this.dispose();
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'second-instance')(() => {
			this.showWindow(windows, lifecycle);
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'activate')(() => {
			this.showWindow(windows, lifecycle);
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'window-all-closed')(() => {
			if (!this.runsInBackground()) {
				app.quit();
			}
		}));
	}

	/** Whether closing the window leaves the application running; see `application.runInBackground`, restricted where no tray icon shows. */
	private runsInBackground(): boolean {
		const configuration = this.instantiationService.invokeFunction(accessor => accessor.get(IConfigurationMainService));
		return !configuration.snapshot.restricted.includes('application.runInBackground') && configuration.getValue('application.runInBackground') !== false;
	}

	private createTray(accessor: ServicesAccessor): void {
		const tray = accessor.get(ITrayMainService);
		const windows = accessor.get(IWindowsMainService);
		const lifecycle = accessor.get(ILifecycleMainService);
		this._register(tray.onDidRequestWindow(() => {
			this.showWindow(windows, lifecycle);
		}));
		tray.create();
	}

	/** Focuses the window, or opens it when the application runs without one. */
	private showWindow(windows: WindowsMainService, lifecycle: LifecycleMainService): void {
		if (!this.restored || lifecycle.quitting) {
			return;
		}
		if (windows.getMainWindow()) {
			windows.focusMainWindow();
			return;
		}
		this.opening ??= this.openWindow().then(() => undefined, (error: unknown) => {
			this.logService.error(error instanceof Error ? error : new Error(String(error)));
		}).finally(() => {
			this.opening = undefined;
		});
	}

	private applyAppearance(accessor: ServicesAccessor): void {
		const configuration = accessor.get(IConfigurationMainService);
		const apply = (): void => {
			const theme = configuration.getValue('appearance.theme');
			nativeTheme.themeSource = theme === 'light' || theme === 'dark' ? theme : 'system';
		};
		apply();
		this._register(configuration.onDidChangeConfiguration(apply));
	}

	private configureSession(): void {
		protocol.handle('vnilink', request => {
			const url = new URL(request.url);
			if (request.method !== 'GET' || url.hostname !== 'app' || url.port || url.username || url.password || !workbenchFiles.has(url.pathname)) {
				return new Response('Not found', { status: 404 });
			}
			return net.fetch(pathToFileURL(join(__dirname, 'renderer', url.pathname.slice(1))).href);
		});
		session.defaultSession.setPermissionRequestHandler((_contents, _permission, callback) => {
			callback(false);
		});
		session.defaultSession.setPermissionCheckHandler(() => false);
	}

	private initChannels(accessor: ServicesAccessor): void {
		const engine = accessor.get(IEngineMainService);
		const windows = accessor.get(IWindowsMainService);
		const nativeHost = accessor.get(INativeHostMainService);
		const permissions = accessor.get(IPermissionsMainService);
		const configuration = accessor.get(IConfigurationMainService);
		const recording = accessor.get(IRecordingMainService);
		const lifecycle = accessor.get(ILifecycleMainService);
		const disposables = this._register(new DisposableStore());
		const server = disposables.add(new ElectronIPCServer());
		server.registerChannel(NATIVE_HOST_CHANNEL_NAME, new NativeHostChannel(nativeHost, async () => {
			// Restarting is only offered while saved relays differ from running ones.
			if (await engine.getRestartRequired()) {
				lifecycle.relaunch();
			}
		}));
		server.registerChannel(ENGINE_CHANNEL_NAME, disposables.add(new EngineChannel(engine, windows, nativeHost, permissions)));
		server.registerChannel(PERMISSIONS_CHANNEL_NAME, new PermissionsChannel(permissions));
		server.registerChannel(CONFIGURATION_CHANNEL_NAME, new ConfigurationChannel(configuration));
		server.registerChannel(RECORDING_CHANNEL_NAME, ProxyChannel.fromService({
			getDefaultDirectory: () => recording.getDefaultDirectory(),
			createDestination: () => recording.createDestination(),
		}, disposables, { disableMarshalling: true }));
		// The preload resolves this before any IPC client exists, as VS Code's window configuration does.
		// eslint-disable-next-line @typescript-eslint/no-deprecated
		validatedIpcMain.handle(WINDOW_CONFIGURATION_CHANNEL, event => Promise.resolve(this.windowConfiguration(event.sender, engine.available)));
		disposables.add({
			dispose: () => {
				// eslint-disable-next-line @typescript-eslint/no-deprecated
				validatedIpcMain.removeHandler(WINDOW_CONFIGURATION_CHANNEL);
			},
		});
	}

	private windowConfiguration(sender: Electron.WebContents, engineAvailable: boolean): INativeWindowConfiguration {
		const window = BrowserWindow.fromWebContents(sender);
		if (!window || (platform !== 'darwin' && platform !== 'win32' && platform !== 'linux')) {
			throw new Error('Unsupported window');
		}
		return {
			windowId: window.id,
			osRelease: release(),
			platform,
			version: app.getVersion(),
			engineAvailable,
			enableSmokeTestDriver: this.options.temporaryEngine === true,
		};
	}

	private async startEngine(): Promise<void> {
		const engine = this.instantiationService.invokeFunction(accessor => accessor.get(IEngineMainService));
		const permissions = this.instantiationService.invokeFunction(accessor => accessor.get(IPermissionsMainService));
		const configuration = this.instantiationService.invokeFunction(accessor => accessor.get(IConfigurationMainService));
		const temporary = this.options.temporaryEngine ?? false;
		try {
			await engine.start(temporary);
			if (!temporary) {
				await permissions.synchronize().catch(() => {
					this.logService.error('Native input backend could not be initialized.');
				});
			}
			await configuration.restoreEngineDefaults(temporary);
			await engine.refreshRestartRequired();
			const runtime = await engine.execute({ type: 'nativeRuntime' });
			if (runtime.type === 'nativeRuntime') {
				for (const backend of runtime.backends) {
					if (!backend.ready) {
						this.logService.error(backend.error ?? 'Native backend unavailable.');
					}
				}
			}
			mark('vnilink/engineReady');
		} catch (error) {
			this.logService.error('VniLink engine unavailable; secure installation could not be opened.', error);
		}
	}

	private async openWindow(): Promise<BrowserWindowType> {
		this.checkRunning();
		const windows = this.instantiationService.invokeFunction(accessor => accessor.get(IWindowsMainService));
		const window = await windows.openMainWindow(this.options.showWindow ?? true);
		registerEditingShortcuts(window);
		registerDevelopmentWindow(window);
		this.checkRunning();
		return window;
	}
}

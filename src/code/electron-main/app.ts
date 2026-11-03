// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { app, BrowserWindow, nativeTheme, net, powerMonitor, protocol, session } from 'electron';
import type { BrowserWindow as BrowserWindowType, WebContents } from 'electron';
import { release } from 'node:os';
import { rm } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { onUnexpectedError, setUnexpectedErrorHandler } from '../../base/common/errors.js';
import { Event } from '../../base/common/event.js';
import { Disposable, DisposableStore, toDisposable } from '../../base/common/lifecycle.js';
import { mark } from '../../base/common/performance.js';
import { isMacintosh } from '../../base/common/platform.js';
import { platform } from '../../base/common/process.js';
import { ProxyChannel } from '../../base/parts/ipc/common/ipc.js';
import { Server as ElectronIPCServer } from '../../base/parts/ipc/electron-main/ipc.electron.js';
import { validatedIpcMain } from '../../base/parts/ipc/electron-main/ipcMain.js';
import { ConfigurationChannel } from '../../platform/configuration/electron-main/configurationChannel.js';
import { ConfigurationMainService, IConfigurationMainService } from '../../platform/configuration/electron-main/configurationMainService.js';
import { launchedAtLogin } from '../../platform/configuration/electron-main/systemSettings.js';
import { ENGINE_CHANNEL_NAME } from '../../platform/engine/common/engineService.js';
import { EngineChannel } from '../../platform/engine/electron-main/engineChannel.js';
import { EngineMainService, IEngineMainService } from '../../platform/engine/electron-main/engineMainService.js';
import { HostingMainService, IHostingMainService } from '../../platform/hosting/electron-main/hostingMainService.js';
import { electronScreenHostingEnvironment, IScreenHostingMainService, ScreenHostingMainService } from '../../platform/hosting/electron-main/screenHostingMainService.js';
import { electronTrayEnvironment, ITrayMainService, TrayMainService } from '../../platform/tray/electron-main/trayMainService.js';
import { ISystemActionNoticeMainService, SystemActionNoticeMainService } from '../../platform/systemActions/electron-main/systemActionNotice.js';
import { traySupported } from '../../platform/tray/electron-main/traySupport.js';
import { SyncDescriptor } from '../../platform/instantiation/common/descriptors.js';
import type { ServicesAccessor } from '../../platform/instantiation/common/instantiation.js';
import { InstantiationService } from '../../platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../platform/instantiation/common/serviceCollection.js';
import { ILifecycleMainService, LifecycleMainService } from '../../platform/lifecycle/electron-main/lifecycleMainService.js';
import { URI } from '../../base/common/uri.js';
import { BufferLogger } from '../../platform/log/common/bufferLog.js';
import { ConsoleMainLogger, ILogService, LogLevel, parseLogLevel } from '../../platform/log/common/log.js';
import { LogService } from '../../platform/log/common/logService.js';
import { LoggerChannel } from '../../platform/log/electron-main/logIpc.js';
import { LOGGER_CHANNEL_NAME } from '../../platform/log/common/loggerChannel.js';
import { WindowLoggerChannel } from '../../platform/log/electron-main/windowLoggerChannel.js';
import { runSelfCheck } from './selfCheck.js';
import { joinShutdown, MainWindowRequests, runsInBackground, sharesScreen, startsWithoutWindow } from './applicationStartup.js';
import { ILoggerMainService, LoggerMainService } from '../../platform/log/electron-main/loggerService.js';
import { logsFolderName, pruneLogs } from '../../platform/log/electron-main/logsFolder.js';
import { installApplicationMenu, registerEditingShortcuts } from '../../platform/menubar/electron-main/menubar.js';
import { registerContextMenuListener } from '../../base/parts/contextmenu/electron-main/contextmenu.js';
import { NATIVE_HOST_CHANNEL_NAME } from '../../platform/native/common/native.js';
import { NativeHostChannel } from '../../platform/native/electron-main/nativeHostChannel.js';
import { INativeHostMainService, NativeHostMainService } from '../../platform/native/electron-main/nativeHostMainService.js';
import { PERMISSIONS_CHANNEL_NAME } from '../../platform/permissions/common/permissions.js';
import { PermissionsChannel } from '../../platform/permissions/electron-main/permissionsChannel.js';
import { IPermissionsMainService, PermissionsMainService } from '../../platform/permissions/electron-main/permissionsMainService.js';
import { APPLICATION_LOCATION_CHANNEL_NAME } from '../../platform/applicationLocation/common/applicationLocation.js';
import { ApplicationLocationChannel } from '../../platform/applicationLocation/electron-main/applicationLocationChannel.js';
import { ApplicationLocationMainService, electronApplicationLocationEnvironment, IApplicationLocationMainService } from '../../platform/applicationLocation/electron-main/applicationLocationMainService.js';
import { INVITATIONS_CHANNEL_NAME } from '../../platform/invitations/common/invitations.js';
import { InvitationsChannel } from '../../platform/invitations/electron-main/invitationsChannel.js';
import { IInvitationsMainService, InvitationsMainService, readInvitation } from '../../platform/invitations/electron-main/invitationsMainService.js';
import { RECORDING_CHANNEL_NAME } from '../../platform/recording/common/recording.js';
import { UI_STATE_CHANNEL_NAME } from '../../platform/state/common/uiState.js';
import { UIStateChannel } from '../../platform/state/electron-main/uiStateChannel.js';
import { IStateService } from '../../platform/state/node/state.js';
import { SaveStrategy, StateService } from '../../platform/state/node/stateService.js';
import { IRecordingMainService, RecordingMainService } from '../../platform/recording/electron-main/recordingMainService.js';
import { CONFIGURATION_CHANNEL_NAME } from '../../platform/configuration/common/configurationSnapshot.js';
import { contentSecurityPolicy } from '../../platform/window/common/contentSecurityPolicy.js';
import { WINDOW_CONFIGURATION_CHANNEL, type INativeWindowConfiguration } from '../../platform/window/common/windowConfiguration.js';
import { IWindowsMainService, WindowsMainService } from '../../platform/windows/electron-main/windowsMainService.js';
import { registerDevelopmentWindow } from './development.js';
import { secureWebContents } from './webContentsSecurity.js';

interface ApplicationOptions {
	readonly showWindow?: boolean;
	readonly temporaryEngine?: boolean;
	/** Lets the temporary engine host this computer's screen, for the latency measurement. */
	readonly temporaryEngineCaptures?: boolean;
	/** Where that engine keeps the desktop portal's grant to share the screen. */
	readonly temporaryEngineScreenGrant?: string;
	readonly onReady?: (window: BrowserWindowType) => Promise<void>;
	/** Exposes the automation driver to the window; only development smoke launches ask for it. */
	readonly automationDriver?: boolean;
	/** Runs `--self-check` once the window is ready, then removes `profile` and exits. */
	readonly selfCheck?: { readonly output: string; readonly profile: string };
}

/** Files the `vnilink://app/` protocol serves: the workbench and the restart notice. Everything else is not found. */
const workbenchFiles = new Set(['/index.html', '/renderer.js', '/renderer.css', '/app-icon.svg', '/notice.html', '/notice.js', '/notice.css']);

/**
 * Main-process composition: creates services, exposes them to windows over
 * IPC and opens the workbench window. Remote capability policy stays in Rust.
 */
export class VniLinkApplication extends Disposable {
	private readonly logService: LogService;
	/** This run's logs folder, `<userData>/logs/<started at>`. */
	private readonly logsHome: URI;
	private readonly loggerService: LoggerMainService;
	private readonly bufferLogger: BufferLogger;
	private readonly stateService: StateService;
	private readonly instantiationService: InstantiationService;
	private started: Promise<void> | undefined;

	constructor(private readonly options: ApplicationOptions = {}) {
		super();
		const logLevel = process.argv.includes('--verbose') ? LogLevel.Trace : LogLevel.Info;
		this.logsHome = URI.file(join(app.getPath('userData'), 'logs', logsFolderName(new Date())));
		this.loggerService = this._register(new LoggerMainService(logLevel, this.logsHome));
		// Main logs to the console at once, and to main.log once this process holds the instance lock.
		this.bufferLogger = new BufferLogger(logLevel);
		this.logService = this._register(new LogService(this.bufferLogger, [new ConsoleMainLogger(logLevel)]));
		setUnexpectedErrorHandler(error => {
			this.logService.error(error instanceof Error ? error : new Error(String(error)));
		});
		// An error nothing caught goes to the log, not to Electron's error dialog.
		const unexpected = (error: unknown): void => {
			onUnexpectedError(error);
		};
		process.on('uncaughtException', unexpected);
		process.on('unhandledRejection', unexpected);
		this._register(toDisposable(() => {
			process.off('uncaughtException', unexpected);
			process.off('unhandledRejection', unexpected);
		}));
		this.stateService = this._register(new StateService(SaveStrategy.DELAYED, join(app.getPath('userData'), 'storage.json'), this.logService));
		this.instantiationService = this._register(new InstantiationService(this.createServices(), true));
	}

	private createServices(): ServiceCollection {
		return new ServiceCollection(
			[ILogService, this.logService],
			[ILoggerMainService, this.loggerService],
			[ILifecycleMainService, new SyncDescriptor(LifecycleMainService, [app, powerMonitor])],
			[IWindowsMainService, new SyncDescriptor(WindowsMainService)],
			[INativeHostMainService, new SyncDescriptor(NativeHostMainService, [this.logsHome])],
			[IEngineMainService, new SyncDescriptor(EngineMainService)],
			[IHostingMainService, new SyncDescriptor(HostingMainService)],
			[IScreenHostingMainService, new SyncDescriptor(ScreenHostingMainService, [electronScreenHostingEnvironment])],
			[ITrayMainService, new SyncDescriptor(TrayMainService, [electronTrayEnvironment])],
			[ISystemActionNoticeMainService, new SyncDescriptor(SystemActionNoticeMainService)],
			[IPermissionsMainService, new SyncDescriptor(PermissionsMainService)],
			[IInvitationsMainService, new SyncDescriptor(InvitationsMainService, [readInvitation])],
			[IApplicationLocationMainService, new SyncDescriptor(ApplicationLocationMainService, [electronApplicationLocationEnvironment])],
			[IConfigurationMainService, new SyncDescriptor(ConfigurationMainService, [join(app.getPath('userData'), 'settings.json')])],
			[IRecordingMainService, new SyncDescriptor(RecordingMainService)],
			[IStateService, this.stateService],
		);
	}

	public start(): Promise<void> {
		this.started ??= this.startup();
		return this.started;
	}

	private async startup(): Promise<void> {
		mark('vnilink/mainStart');
		if (!app.requestSingleInstanceLock()) {
			// A second launch only wakes the first: it writes no logs and prunes none of the first's.
			this.dispose();
			app.quit();
			return;
		}
		this.bufferLogger.logger = this.loggerService.createLogger('main', { name: 'Main' });
		void pruneLogs(join(app.getPath('userData'), 'logs'), this.logsHome.fsPath).catch((error: unknown) => {
			this.logService.warn('Older logs could not be removed', error);
		});
		protocol.registerSchemesAsPrivileged([{ scheme: 'vnilink', privileges: { standard: true, secure: true, supportFetchAPI: true } }]);
		const windowRequests = this.instantiationService.invokeFunction(accessor => this.registerListeners(accessor));
		try {
			await app.whenReady();
			this.checkRunning();
			this.instantiationService.invokeFunction(accessor => accessor.get(ILifecycleMainService)).listenForSystemShutdown();
			installApplicationMenu();
			// The window's native context menus, which macOS uses.
			registerContextMenuListener();
			await Promise.all([
				this.instantiationService.invokeFunction(accessor => accessor.get(IConfigurationMainService)).initialize(),
				this.stateService.init(),
			]);
			this.checkRunning();
			this.instantiationService.invokeFunction(accessor => {
				this.applyAppearance(accessor);
				this.applyLogLevel(accessor);
				this.configureSession();
				this.initChannels(accessor);
				// Windows and Linux start the application with the invitation the user opened.
				void accessor.get(IInvitationsMainService).openFromCommandLine(process.argv.slice(1), process.cwd());
			});
			mark('vnilink/appReady');
			await this.startEngine();
			this.checkRunning();
			this.instantiationService.invokeFunction(accessor => {
				accessor.get(IHostingMainService).start();
				if (sharesScreen(this.options)) {
					void accessor.get(IScreenHostingMainService).start();
				}
				// A paired computer may ask this one to restart or shut down; its user sees and answers it.
				accessor.get(ISystemActionNoticeMainService).start();
			});
			if (await traySupported()) {
				this.checkRunning();
				this.instantiationService.invokeFunction(accessor => {
					this.createTray(accessor, windowRequests);
				});
			}
			if (startsWithoutWindow(this.options.temporaryEngine ?? false, this.runsInBackground(), () => launchedAtLogin())) {
				// Started by the login item: accept paired devices without a window until the user opens one.
				windowRequests.startedWithoutWindow();
				return;
			}
			// A macOS application opened from the disk image or Downloads offers to move to Applications, which relaunches it.
			if (await this.instantiationService.invokeFunction(accessor => accessor.get(IApplicationLocationMainService)).offerAtStartup()) {
				return;
			}
			const window = await this.openWindow();
			windowRequests.openedAtStartup();
			mark('vnilink/windowReady');
			if (this.options.selfCheck) {
				await this.runSelfCheck(window, this.options.selfCheck);
				return;
			}
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

	private async runSelfCheck(window: BrowserWindowType, { output, profile }: { readonly output: string; readonly profile: string }): Promise<void> {
		const engine = this.instantiationService.invokeFunction(accessor => accessor.get(IEngineMainService));
		const lifecycle = this.instantiationService.invokeFunction(accessor => accessor.get(ILifecycleMainService));
		const result = await runSelfCheck(window, engine, output);
		process.stdout.write(`${JSON.stringify(result)}\n`);
		await lifecycle.runShutdown();
		this.dispose();
		await rm(profile, { recursive: true, force: true }).catch(() => undefined);
		app.exit(result.passed ? 0 : 1);
	}

	private checkRunning(): void {
		const lifecycle = this.instantiationService.invokeFunction(accessor => accessor.get(ILifecycleMainService));
		if (lifecycle.quitting || this._store.isDisposed) {
			throw new Error('Application stopped during startup');
		}
	}

	/** Registers the application's listeners and returns what answers requests to see the window. */
	private registerListeners(accessor: ServicesAccessor): MainWindowRequests {
		const lifecycle = accessor.get(ILifecycleMainService);
		const windows = accessor.get(IWindowsMainService);
		const engine = accessor.get(IEngineMainService);
		const configuration = accessor.get(IConfigurationMainService);
		const state = accessor.get(IStateService);
		const windowRequests = new MainWindowRequests(windows, lifecycle, () => this.openWindow(), this.logService);
		this._register(joinShutdown(lifecycle.onWillShutdown, { engine, configuration, state }));
		this._register(Event.fromNodeEventEmitter<WebContents>(app, 'web-contents-created', (_event: unknown, contents: WebContents) => contents)(secureWebContents));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'will-quit')(() => {
			this.dispose();
		}));
		const invitations = accessor.get(IInvitationsMainService);
		// Another launch passes on its command line, which names the invitation opened on Windows and Linux.
		this._register(Event.fromNodeEventEmitter<{ argv: string[]; workingDirectory: string }>(app, 'second-instance', (_event: unknown, argv: string[], workingDirectory: string) => ({ argv, workingDirectory }))(launch => {
			void invitations.openFromCommandLine(launch.argv.slice(1), launch.workingDirectory);
			windowRequests.request();
		}));
		// macOS opens an invitation in the running application, before `ready` when it starts it.
		this._register(Event.fromNodeEventEmitter<string>(app, 'open-file', (event: { preventDefault(): void }, path: string) => {
			event.preventDefault();
			return path;
		})(path => {
			void invitations.openPaths([path]);
			windowRequests.request();
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'activate')(() => {
			windowRequests.request();
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'window-all-closed')(() => {
			if (!this.runsInBackground()) {
				app.quit();
			}
		}));
		return windowRequests;
	}

	private runsInBackground(): boolean {
		const configuration = this.instantiationService.invokeFunction(accessor => accessor.get(IConfigurationMainService));
		return runsInBackground(configuration.snapshot.restricted, configuration.getValue('application.runInBackground'));
	}

	private createTray(accessor: ServicesAccessor, windowRequests: MainWindowRequests): void {
		const tray = accessor.get(ITrayMainService);
		this._register(tray.onDidRequestWindow(() => {
			windowRequests.request();
		}));
		tray.create();
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

	/**
	 * The application's own logs follow `diagnostics.applicationLog`;
	 * `--verbose` keeps everything. The engine's log has its own setting.
	 */
	private applyLogLevel(accessor: ServicesAccessor): void {
		if (process.argv.includes('--verbose')) {
			return;
		}
		const configuration = accessor.get(IConfigurationMainService);
		const apply = (): void => {
			const value = configuration.getValue('diagnostics.applicationLog');
			const level = (typeof value === 'string' ? parseLogLevel(value) : undefined) ?? LogLevel.Info;
			if (level !== this.loggerService.getLogLevel()) {
				this.loggerService.setLogLevel(level);
				this.logService.setLevel(level);
			}
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
			return net.fetch(pathToFileURL(join(__dirname, 'renderer', url.pathname.slice(1))).href).then(response => {
				const headers = new Headers(response.headers);
				headers.set('Content-Security-Policy', contentSecurityPolicy);
				return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
			});
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
		server.registerChannel(INVITATIONS_CHANNEL_NAME, new InvitationsChannel(accessor.get(IInvitationsMainService)));
		server.registerChannel(APPLICATION_LOCATION_CHANNEL_NAME, new ApplicationLocationChannel(accessor.get(IApplicationLocationMainService)));
		server.registerChannel(LOGGER_CHANNEL_NAME, new WindowLoggerChannel(disposables.add(new LoggerChannel(this.loggerService)), this.logsHome));
		// A closed window's loggers, and their open files, go with it.
		disposables.add(windows.onDidOpenWindow(window => {
			const windowId = window.id;
			window.once('closed', () => {
				this.loggerService.deregisterLoggers(windowId);
			});
		}));
		server.registerChannel(CONFIGURATION_CHANNEL_NAME, new ConfigurationChannel(configuration));
		server.registerChannel(UI_STATE_CHANNEL_NAME, new UIStateChannel(accessor.get(IStateService)));
		server.registerChannel(RECORDING_CHANNEL_NAME, ProxyChannel.fromService({
			getDefaultDirectory: () => recording.getDefaultDirectory(),
			createDestination: () => recording.createDestination(),
			savePicture: (name: unknown, png: unknown) => recording.savePicture(name, png),
		}, disposables, { disableMarshalling: true }));
		// The preload resolves this before any IPC client exists.
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
			enableSmokeTestDriver: this.options.automationDriver === true,
			logsPath: this.logsHome.fsPath,
			logLevel: this.loggerService.getLogLevel(),
		};
	}

	private async startEngine(): Promise<void> {
		const engine = this.instantiationService.invokeFunction(accessor => accessor.get(IEngineMainService));
		const temporary = this.options.temporaryEngine ?? false;
		engine.setRestartHandler(() => this.initializeEngine(temporary));
		try {
			await engine.start(temporary, temporary && this.options.temporaryEngineCaptures === true, this.options.temporaryEngineScreenGrant);
		} catch (error) {
			// The engine tries again by itself; its restart handler configures it then.
			this.logService.error('The engine could not be started; it will be tried again.', error);
			return;
		}
		try {
			await this.initializeEngine(temporary);
			mark('vnilink/engineReady');
		} catch (error) {
			this.logService.error('The engine started, but main could not configure it.', error);
		}
	}

	/** Applies what main configures in a new engine; it runs after the first start and after each restart. */
	private async initializeEngine(temporary: boolean): Promise<void> {
		const engine = this.instantiationService.invokeFunction(accessor => accessor.get(IEngineMainService));
		const permissions = this.instantiationService.invokeFunction(accessor => accessor.get(IPermissionsMainService));
		const configuration = this.instantiationService.invokeFunction(accessor => accessor.get(IConfigurationMainService));
		// Transfers stage here; the engine creates it the first time one does.
		await engine.execute({ type: 'setTransferDirectory', path: join(app.getPath('userData'), 'Transfers') }).catch((error: unknown) => {
			this.logService.error('The transfer folder could not be set; Files transfers will fail.', error);
		});
		if (!temporary) {
			await permissions.synchronize().catch(() => {
				this.logService.error('Native input backend could not be initialized.');
			});
		} else if (this.options.temporaryEngineCaptures === true && isMacintosh) {
			// Screen only: a measuring host is viewed, never controlled.
			// Elsewhere the system asks no permission to capture, and the
			// engine is told nothing, as `synchronizeNativePermissions` does.
			await engine.execute({ type: 'reportNativePermissions', screen: permissions.isGranted('screen'), input: false, camera: false, microphone: false });
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
	}

	private async openWindow(): Promise<BrowserWindowType> {
		this.checkRunning();
		const windows = this.instantiationService.invokeFunction(accessor => accessor.get(IWindowsMainService));
		const window = await windows.openMainWindow(this.options.showWindow ?? true);
		const nativeHost = this.instantiationService.invokeFunction(accessor => accessor.get(INativeHostMainService));
		registerEditingShortcuts(window, () => nativeHost.hasRemoteKeyboard(window.id));
		registerDevelopmentWindow(window);
		this.checkRunning();
		return window;
	}
}

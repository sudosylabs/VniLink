import { Event } from '../../base/common/event.js';
import { Disposable } from '../../base/common/lifecycle.js';
import { platform as safePlatform } from '../../base/common/process.js';
import { synchronizeNativePermissions } from '../../platform/engine/electron-main/nativeRuntime.js';
import { MachinePermissionService } from '../../platform/permissions/electron-main/permissionService.js';
import { mark } from '../../base/common/performance.js';
import { isMacintosh } from '../../base/common/platform.js';
import { InstantiationService } from '../../platform/instantiation/common/instantiationService.js';
import { LogService } from '../../platform/log/common/logService.js';
import { ConsoleLogger, LogLevel } from '../../platform/log/common/log.js';
import { ServiceCollection } from '../../platform/instantiation/common/serviceCollection.js';
import { SyncDescriptor } from '../../platform/instantiation/common/descriptors.js';
import { INativeHostMainService, NativeHostMainService } from '../../platform/native/electron-main/nativeHostMainService.js';
import { EngineService } from '../../platform/engine/electron-main/engineService.js';
import { EncryptionMainService } from '../../platform/encryption/electron-main/encryptionMainService.js';
import { app, BrowserWindow, nativeTheme, net, protocol, session, systemPreferences } from 'electron';
import { ConfigurationStore } from '../../platform/configuration/node/configurationStore.js';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createMainWindow } from './windows.js';
import { registerDevelopmentWindow } from './development.js';
import { registerDesktopHandlers } from '../../platform/ipc/electron-main/handlers.js';

export const applicationUrl = 'vnilink://app/index.html';

export interface ApplicationOptions {
	showWindow?: boolean;
	temporaryEngine?: boolean;
	onReady?: (window: BrowserWindow) => Promise<void>;
}

/** Main-process composition and shutdown owner. Remote capability policy remains in Rust. */
export class VniLinkApplication extends Disposable {
	private window: BrowserWindow | null = null;
	private readonly logService: LogService;
	private readonly engine = new EngineService();
	private readonly encryption: EncryptionMainService;
	private readonly configuration: ConfigurationStore;
	private readonly services: InstantiationService;
	private readonly nativeHost: NativeHostMainService;
	private quitting = false;
	private quitReady = false;
	private restored = false;
	private restarting = false;
	private started: Promise<void> | undefined;
	private stopped: Promise<void> | undefined;

	constructor(private readonly options: ApplicationOptions = {}) {
		super();
		this.logService = new LogService(new ConsoleLogger(process.argv.includes('--verbose') ? LogLevel.Trace : LogLevel.Info));
		this.encryption = new EncryptionMainService(this.logService);
		this.configuration = this._register(new ConfigurationStore(join(app.getPath('userData'), 'settings.json')));
		this.services = this._register(new InstantiationService(new ServiceCollection([INativeHostMainService, new SyncDescriptor(NativeHostMainService, [() => this.window])]), true));
		this.nativeHost = this.services.invokeFunction(accessor => accessor.get(INativeHostMainService));
	}

	public start(): Promise<void> {
		this.started ??= this.startup();
		return this.started;
	}

	private async startup(): Promise<void> {
		this.checkRunning();
		mark('vnilink/mainStart');
		app.setName('VniLink');
		if (!app.requestSingleInstanceLock()) {
			this.dispose();
			app.quit();
			return;
		}
		protocol.registerSchemesAsPrivileged([{ scheme: 'vnilink', privileges: { standard: true, secure: true, supportFetchAPI: true } }]);
		this.registerListeners();
		try {
			await app.whenReady();
			this.checkRunning();
			await this.configuration.initialize();
			this.checkRunning();
			this.applyConfiguration();
			this._register(this.configuration.onDidChangeConfiguration(() => {
				this.applyConfiguration();
			}));
			this.configureSession();
			this.registerHandlers();
			mark('vnilink/appReady');
			await this.startEngine();
			this.checkRunning();
			const window = await this.openWindow();
			this.restored = true;
			mark('vnilink/windowReady');
			await this.options.onReady?.(window);
		} catch (error) {
			if (this.quitting || this._store.isDisposed) {
				return;
			}
			this.logService.error(error instanceof Error ? error : new Error(String(error)));
			await this.shutdown();
			this.dispose();
			app.exit(1);
		}
	}

	private checkRunning(): void {
		if (this.quitting || this._store.isDisposed) {
			throw new Error('Application stopped during startup');
		}
	}

	private registerListeners(): void {
		this._register(this.nativeHost.onDidChangeWindowState(state => {
			const window = this.window;
			if (window && !window.isDestroyed() && state.windowId === window.id && window.webContents.mainFrame.url === applicationUrl) {
				window.webContents.send('nativeHost:state', state);
			}
		}));
		this._register(Event.fromNodeEventEmitter<Electron.Event>(app, 'before-quit')(event => {
			this.onBeforeQuit(event);
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'will-quit')(() => {
			this.dispose();
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'second-instance')(() => {
			this.focusWindow();
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'activate')(() => {
			if (this.restored && !this.quitting && BrowserWindow.getAllWindows().length === 0) {
				void this.openWindow().catch((error: unknown) => {
					this.logService.error(error instanceof Error ? error : new Error(String(error)));
				});
			}
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(app, 'window-all-closed')(() => {
			if (!isMacintosh) {
				app.quit();
			}
		}));
	}

	private configureSession(): void {
		const allowed = new Set(['/index.html', '/renderer.js', '/renderer.css', '/app-icon.svg']);
		protocol.handle('vnilink', request => {
			const url = new URL(request.url);
			if (request.method !== 'GET' || url.hostname !== 'app' || url.port || url.username || url.password || !allowed.has(url.pathname)) {
				return new Response('Not found', { status: 404 });
			}
			return net.fetch(pathToFileURL(join(__dirname, 'renderer', url.pathname.slice(1))).href);
		});
		session.defaultSession.setPermissionRequestHandler((_contents, _permission, callback) => {
			callback(false);
		});
		session.defaultSession.setPermissionCheckHandler(() => false);
	}

	private registerHandlers(): void {
		this._register(registerDesktopHandlers(() => this.window, applicationUrl, this.engine, this.nativeHost, this.encryption, this.configuration, () => {
			this.requestRestart();
		}));
	}

	private applyConfiguration(): void {
		nativeTheme.themeSource = this.configuration.snapshot.values['appearance.theme'];
		this.nativeHost.setNativeBackgroundEnabled(this.configuration.snapshot.values['appearance.nativeBackground']);
		const window = this.window;
		if (window && !window.isDestroyed() && window.webContents.mainFrame.url === applicationUrl) {
			window.webContents.send('configuration:changed', this.configuration.snapshot);
		}
	}

	private async startEngine(): Promise<void> {
		try {
			await this.engine.start(this.options.temporaryEngine ?? false);
			await this.restoreEngineDefaults();
			const consent = new MachinePermissionService(safePlatform, !app.isPackaged, systemPreferences, () => Promise.reject(new Error('Startup cannot open permission settings')));
			if (!this.options.temporaryEngine) {
				await synchronizeNativePermissions(this.engine, safePlatform, consent.snapshot()).catch(() => {
					this.logService.error('Native input backend could not be initialized.');
				});
			}
			const runtime = await this.engine.execute({ type: 'nativeRuntime' });
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

	private async restoreEngineDefaults(): Promise<void> {
		const values = this.configuration.snapshot.values;
		await this.engine.execute({ type: 'setDesktopCodecDefault', codec: values['desktop.codec'] });
		await this.engine.execute({ type: 'setDesktopQualityDefaults', quality: values['desktop.quality'] });
		await this.engine.execute({ type: 'setInputDefaults', ...values['input.defaults'] });
		try {
			const paths = values['files.roots'];
			await this.engine.execute(paths.length || this.options.temporaryEngine ? { type: 'setFileRoots', paths: [...paths] } : { type: 'enableFilesystemAccess' });
		} catch (error) {
			this.logService.error('Shared folders could not be opened; file access remains disabled.', error);
		}
	}

	private async openWindow(): Promise<BrowserWindow> {
		this.checkRunning();
		const window = createMainWindow();
		registerDevelopmentWindow(window);
		this.window = window;
		this.nativeHost.registerWindow(window);
		window.once('closed', () => {
			if (this.window === window) {
				this.window = null;
			}
		});
		try {
			await window.loadURL(applicationUrl);
			if (this.quitting) {
				throw new Error('Application stopped during window startup');
			}
			if (this.options.showWindow ?? true) {
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

	private focusWindow(): void {
		if (this.window?.isMinimized()) {
			this.window.restore();
		}
		this.window?.show();
		this.window?.focus();
	}

	private requestRestart(): void {
		if (this.restarting) {
			return;
		}
		this.restarting = true;
		if (app.isPackaged || process.env.VNILINK_DEV !== '1') {
			app.relaunch();
		}
		app.quit();
	}

	private onBeforeQuit(event: Electron.Event): void {
		if (this.quitReady) {
			return;
		}
		event.preventDefault();
		if (this.quitting) {
			return;
		}
		this.quitting = true;
		void this.shutdown().finally(() => {
			this.quitReady = true;
			if (this.restarting && process.env.VNILINK_DEV === '1' && !app.isPackaged) {
				app.exit(42);
			} else {
				app.quit();
			}
		});
	}

	private shutdown(): Promise<void> {
		this.quitting = true;
		this.stopped ??= this.shutdownServices();
		return this.stopped;
	}

	private async shutdownServices(): Promise<void> {
		const results = await Promise.allSettled([this.engine.stop(), this.configuration.flush()]);
		for (const result of results) {
			if (result.status === 'rejected') {
				this.logService.error('Application shutdown failed', result.reason);
			}
		}
	}

	public override dispose(): void {
		super.dispose();
		this.logService.dispose();
	}
}

/** Compatibility entry point for existing callers; the application instance owns all state. */
export function startApplication(options: ApplicationOptions = {}): Promise<void> {
	return new VniLinkApplication(options).start();
}

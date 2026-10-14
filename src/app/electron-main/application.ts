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

interface ApplicationOptions {
	showWindow?: boolean;
	temporaryEngine?: boolean;
	onReady?: (window: BrowserWindow) => Promise<void>;
}

export async function startApplication(options: ApplicationOptions = {}): Promise<void> {
	mark('vnilink/mainStart');
	// `--verbose` maps to trace until the environment platform owns log levels.
	const logService = new LogService(new ConsoleLogger(process.argv.includes('--verbose') ? LogLevel.Trace : LogLevel.Info));
	app.setName('VniLink');
	if (!app.requestSingleInstanceLock()) {
		app.quit();
		return;
	}
	protocol.registerSchemesAsPrivileged([{ scheme: 'vnilink', privileges: { standard: true, secure: true, supportFetchAPI: true } }]);
	let window: BrowserWindow | null = null;
	const engine = new EngineService();
	const encryption = new EncryptionMainService(logService);
	const configuration = new ConfigurationStore(join(app.getPath('userData'), 'settings.json'));
	const services = new InstantiationService(new ServiceCollection([INativeHostMainService, new SyncDescriptor(NativeHostMainService, [() => window])]), true);
	const nativeHost = services.invokeFunction(accessor => accessor.get(INativeHostMainService));
	nativeHost.onDidChangeWindowState(state => {
		if (window && !window.isDestroyed() && state.windowId === window.id && window.webContents.mainFrame.url === applicationUrl) {
			window.webContents.send('nativeHost:state', state);
		}
	});
	app.on('will-quit', () => {
		services.dispose();
		configuration.dispose();
		logService.dispose();
	});
	app.on('second-instance', () => {
		if (window?.isMinimized()) {
			window.restore();
		}
		window?.show();
		window?.focus();
	});
	let quitting = false;
	let restarting = false;
	app.on('before-quit', event => {
		if (quitting) {
			return;
		}
		event.preventDefault();
		quitting = true;
		void Promise.all([engine.stop(), configuration.flush()]).catch(() => {
			logService.error('Engine shutdown failed');
		}).finally(() => {
			if (restarting && process.env.VNILINK_DEV === '1' && !app.isPackaged) {
				app.exit(42);
			} else {
				app.quit();
			}
		});
	});
	async function openWindow(): Promise<BrowserWindow> {
		const current = createMainWindow();
		registerDevelopmentWindow(current);
		window = current;
		nativeHost.registerWindow(current);
		current.on('closed', () => {
			if (window === current) {
				window = null;
			}
		});
		await current.loadURL(applicationUrl);
		if (options.showWindow ?? true) {
			current.show();
		}
		return current;
	}
	app.on('activate', () => {
		if (BrowserWindow.getAllWindows().length === 0) {
			void openWindow().catch((error: unknown) => {
				logService.error(error instanceof Error ? error : new Error(String(error)));
			});
		}
	});
	app.on('window-all-closed', () => {
		if (!isMacintosh) {
			app.quit();
		}
	});
	try {
		await app.whenReady();
		await configuration.initialize();
		const applyConfiguration = (): void => {
			nativeTheme.themeSource = configuration.snapshot.values['appearance.theme'];
			nativeHost.setNativeBackgroundEnabled(configuration.snapshot.values['appearance.nativeBackground']);
			if (window && !window.isDestroyed() && window.webContents.mainFrame.url === applicationUrl) {
				window.webContents.send('configuration:changed', configuration.snapshot);
			}
		};
		applyConfiguration();
		configuration.onDidChangeConfiguration(applyConfiguration);
		mark('vnilink/appReady');
		protocol.handle('vnilink', (request) => {
			const url = new URL(request.url);
			const allowed = new Set(['/index.html', '/renderer.js', '/renderer.css', '/app-icon.svg']);
			if (request.method !== 'GET' || url.hostname !== 'app' || url.port || url.username || url.password || !allowed.has(url.pathname)) {
				return new Response('Not found', { status: 404 });
			}
			return net.fetch(pathToFileURL(join(__dirname, 'renderer', url.pathname.slice(1))).href);
		});
		session.defaultSession.setPermissionRequestHandler((_contents, _permission, callback) => {
			callback(false);
		});
		session.defaultSession.setPermissionCheckHandler(() => false);
		registerDesktopHandlers(() => window, applicationUrl, engine, nativeHost, encryption, configuration, () => {
			restarting = true;
			if (app.isPackaged || process.env.VNILINK_DEV !== '1') {
				app.relaunch();
			}
			app.quit();
		});
		try {
			await engine.start(options.temporaryEngine ?? false);
			await engine.execute({ type: 'setDesktopCodecDefault', codec: configuration.snapshot.values['desktop.codec'] });
			await engine.execute({ type: 'setDesktopQualityDefaults', quality: configuration.snapshot.values['desktop.quality'] });
			await engine.execute({ type: 'setInputDefaults', ...configuration.snapshot.values['input.defaults'] });
			try {
				const roots = configuration.snapshot.values['files.roots'];
				await engine.execute(roots.length || options.temporaryEngine ? { type: 'setFileRoots', paths: [...roots] } : { type: 'enableFilesystemAccess' });
			} catch {
				logService.error('Shared folders could not be opened; file access remains disabled.');
			}
			const consent = new MachinePermissionService(safePlatform, !app.isPackaged, systemPreferences, () => Promise.reject(new Error('Startup cannot open permission settings')));
			if (!options.temporaryEngine) {
				await synchronizeNativePermissions(engine, safePlatform, consent.snapshot()).catch(() => {
					logService.error('Native input backend could not be initialized.');
				});
			}
			const runtime = await engine.execute({ type: 'nativeRuntime' });
			if (runtime.type === 'nativeRuntime') {
				for (const backend of runtime.backends) {
					if (!backend.ready) {
						logService.error(backend.error ?? 'Native backend unavailable.');
					}
				}
			}
			mark('vnilink/engineReady');
		} catch {
			logService.error('VniLink engine unavailable; secure installation could not be opened.');
		}
		const current = await openWindow();
		mark('vnilink/windowReady');
		await options.onReady?.(current);
	} catch (error) {
		logService.error(error instanceof Error ? error : new Error(String(error)));
		app.exit(1);
	}
}

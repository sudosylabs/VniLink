import { EngineService } from '../../platform/engine/electron-main/engineService.js';
import { app, BrowserWindow, net, protocol, session } from 'electron';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createMainWindow } from './windows.js';
import { registerDesktopHandlers } from '../../platform/ipc/electron-main/handlers.js';
export const applicationUrl = 'vnilink://app/index.html';
interface ApplicationOptions {
	showWindow?: boolean;
	temporaryEngine?: boolean;
	onReady?: (window: BrowserWindow) => Promise<void>;
}
export async function startApplication(options: ApplicationOptions = {}): Promise<void> {
	app.setName('VniLink');
	if (!app.requestSingleInstanceLock()) {
		app.quit(); return;
	}
	protocol.registerSchemesAsPrivileged([{ scheme: 'vnilink', privileges: { standard: true, secure: true, supportFetchAPI: true } }]);
	let window: BrowserWindow | null = null;
	const engine = new EngineService();
	app.on('second-instance', () => {
		if (window?.isMinimized()) {
			window.restore();
		}
		window?.show(); window?.focus();
	});
	let quitting = false;
	app.on('before-quit', event => {
		if (quitting) {
			return;
		}
		event.preventDefault();
		quitting = true;
		void engine.stop().catch(() => {
			console.error('Engine shutdown failed');
		}).finally(() => {
			app.quit();
		});
	});
	async function openWindow(): Promise<BrowserWindow> {
		const current = createMainWindow();
		window = current;
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
				console.error(error);
			});
		}
	});
	app.on('window-all-closed', () => {
		if (process.platform !== 'darwin') {
			app.quit();
		}
	});
	try {
		await app.whenReady();
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
		registerDesktopHandlers(() => window, applicationUrl, engine);
		try {
			await engine.start(options.temporaryEngine ?? false);
		} catch {
			console.error('VniLink engine unavailable; secure installation could not be opened.');
		}
		const current = await openWindow();
		await options.onReady?.(current);
	} catch (error) {
		console.error(error); app.exit(1);
	}
}

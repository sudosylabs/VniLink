import { defaultBrowserWindowOptions } from '../../platform/windows/electron-main/windows.js';
import { isMacintosh, isLinux } from '../../base/common/platform.js';
import { app, BrowserWindow } from 'electron';
import { join } from 'node:path';

export function createMainWindow(): BrowserWindow {
	const options = defaultBrowserWindowOptions();
	const window = new BrowserWindow({ ...options, ...(isLinux && app.isPackaged ? { icon: join(process.resourcesPath, 'app-icon.png') } : {}), webPreferences: { ...options.webPreferences, preload: join(__dirname, 'preload.cjs') } });
	if (!isMacintosh) {
		window.removeMenu();
	}
	window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
	window.webContents.on('will-navigate', (event) => {
		event.preventDefault();
	});
	window.webContents.on('will-redirect', (event) => {
		event.preventDefault();
	});
	return window;
}

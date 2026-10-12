import { defaultBrowserWindowOptions } from '../../platform/windows/electron-main/windows.js';
import { BrowserWindow } from 'electron';
import { join } from 'node:path';
export function createMainWindow(): BrowserWindow {
	const options = defaultBrowserWindowOptions();
	const window = new BrowserWindow({ ...options, webPreferences: { ...options.webPreferences, preload: join(__dirname, 'preload.cjs') } });
	if (process.platform !== 'darwin') {
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

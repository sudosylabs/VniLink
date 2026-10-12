import { BrowserWindow } from 'electron';
import { join } from 'node:path';
export function createMainWindow(): BrowserWindow {
	const window = new BrowserWindow({
		title: 'VniLink', width: 1120, height: 760, minWidth: 760, minHeight: 520,
		show: false, backgroundColor: '#f7f7f8', titleBarStyle: 'hidden', frame: process.platform === 'darwin',
		...(process.platform === 'darwin' ? { trafficLightPosition: { x: 18, y: 18 } } : {}),
		webPreferences: { preload: join(__dirname, 'preload.cjs'), sandbox: true, contextIsolation: true, nodeIntegration: false, webviewTag: false },
	});
	window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
	window.webContents.on('will-navigate', (event) => {
		event.preventDefault();
	});
	window.webContents.on('will-redirect', (event) => {
		event.preventDefault();
	});
	return window;
}

/**
 * The perceived-quality stimulus window (`npm run measure:perceived`): the
 * page in `stimulus.ts`, full screen above everything on the computer being
 * controlled, until the runner stops this process. It needs nothing of
 * VniLink's own: the remote desktop application under test only has to show
 * this screen.
 */
import { app, BrowserWindow } from 'electron';
import { perceivedPage } from './stimulus.js';

app.whenReady().then(async () => {
	const window = new BrowserWindow({
		fullscreen: true,
		frame: false,
		show: false,
		backgroundColor: '#ffffff',
		webPreferences: { backgroundThrottling: false },
	});
	window.setAlwaysOnTop(true, 'screen-saver');
	await window.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(perceivedPage())}`);
	window.show();
}).catch((error: unknown) => {
	console.error(error);
	app.exit(1);
});

app.on('window-all-closed', () => {
	app.quit();
});

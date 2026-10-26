/**
 * The perceived-quality stimulus window (`npm run measure:perceived`): the
 * page in `stimulus.ts`, maximised above everything, until the runner stops
 * this process. It runs on the Mac doing the measuring by default, or on the
 * second computer being controlled with `--remote`. It needs nothing of
 * VniLink's own: a remote desktop application under test only has to show
 * this screen.
 *
 * Maximised, not full screen: on GNOME a full-screen window stops RustDesk's
 * screen capture after one picture, and the page must reach every
 * application under test. The reader finds the page wherever it is.
 */
import { app, BrowserWindow } from 'electron';
import { perceivedPage } from './stimulus.js';

app.whenReady().then(async () => {
	const window = new BrowserWindow({
		frame: false,
		show: false,
		backgroundColor: '#ffffff',
		webPreferences: { backgroundThrottling: false },
	});
	window.setAlwaysOnTop(true, 'screen-saver');
	await window.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(perceivedPage())}`);
	window.maximize();
	window.show();
	// The driver's keys go to the active application: started from a
	// terminal, this one would otherwise stay behind it.
	app.focus({ steal: true });
}).catch((error: unknown) => {
	console.error(error);
	app.exit(1);
});

app.on('window-all-closed', () => {
	app.quit();
});

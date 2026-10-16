import { DesktopMain } from './workbench/electron-browser/desktop.main.js';

void window.vnilink.getAppInfo().then(async info => {
	const desktop = new DesktopMain(window.vnilink, info);
	await desktop.open();
}).catch((error: unknown) => {
	console.error(error);
	const status = document.querySelector('[role="status"]');
	if (status) {
		status.textContent = 'The desktop could not start. Close the window and try again.';
	}
});

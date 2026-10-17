import { context } from './base/parts/sandbox/electron-browser/globals.js';
import { isNativeWindowConfiguration } from './platform/window/common/windowConfiguration.js';
import { DesktopMain } from './workbench/electron-browser/desktop.main.js';

void (async () => {
	const configuration: unknown = await context.resolveConfiguration();
	if (!isNativeWindowConfiguration(configuration)) {
		throw new Error('Invalid window configuration');
	}
	await new DesktopMain(configuration).open();
})().catch((error: unknown) => {
	console.error(error);
	const status = document.querySelector('[role="status"]');
	if (status) {
		status.textContent = 'The desktop could not start. Close the window and try again.';
	}
});

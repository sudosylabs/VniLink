import type { BrowserWindow } from 'electron';
import { isMacintosh } from '../../base/common/platform.js';
import { isBuilt } from '../../platform/environment/electron-main/environment.js';

/** Matches VS Code's development-only Reload Window and Toggle Developer Tools shortcuts. */
export function registerDevelopmentWindow(window: BrowserWindow): void {
	if (isBuilt() || process.env.VNILINK_DEV !== '1') {
		return;
	}
	window.webContents.on('before-input-event', (event, input) => {
		const modifier = isMacintosh ? input.meta : input.control;
		if (input.type !== 'keyDown' || !modifier) {
			return;
		}
		const key = input.key.toLowerCase();
		if (!input.alt && !input.shift && key === 'r') {
			event.preventDefault();
			window.webContents.reloadIgnoringCache();
		} else if ((isMacintosh ? input.alt && !input.shift : input.shift && !input.alt) && (key === 'i' || input.code === 'KeyI')) {
			event.preventDefault();
			window.webContents.toggleDevTools();
		}
	});
}

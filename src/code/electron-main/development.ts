import type { BrowserWindow } from 'electron';
import { isMacintosh } from '../../base/common/platform.js';
import { isBuilt } from '../../platform/environment/electron-main/environment.js';

/** Matches VS Code's development-only Reload Window shortcut. */
export function registerDevelopmentWindow(window: BrowserWindow): void {
	if (isBuilt() || process.env.VNILINK_DEV !== '1') {
		return;
	}
	window.webContents.on('before-input-event', (event, input) => {
		const modifier = isMacintosh ? input.meta : input.control;
		if (input.type === 'keyDown' && modifier && !input.alt && !input.shift && input.key.toLowerCase() === 'r') {
			event.preventDefault();
			window.webContents.reloadIgnoringCache();
		}
	});
}

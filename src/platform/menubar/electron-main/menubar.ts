import { Menu, MenuItem } from 'electron';
import type { BrowserWindow } from 'electron';
import { isMacintosh } from '../../../base/common/platform.js';
import * as nls from '../../../nls.js';
import { product } from '../../product/common/product.js';

/**
 * Replaces Electron's default menu. macOS shows only the application menu, as
 * in VS Code's `setMacApplicationMenu`; other platforms have no menu because
 * the workbench draws its own title bar.
 */
export function installApplicationMenu(): void {
	if (!isMacintosh) {
		Menu.setApplicationMenu(null);
		return;
	}
	const application = new Menu();
	for (const item of [
		new MenuItem({ label: nls.localize('mAbout', 'About {0}', product.name), role: 'about' }),
		new MenuItem({ type: 'separator' }),
		new MenuItem({ label: nls.localize('mServices', 'Services'), role: 'services', submenu: new Menu() }),
		new MenuItem({ type: 'separator' }),
		new MenuItem({ label: nls.localize('mHide', 'Hide {0}', product.name), role: 'hide', accelerator: 'Command+H' }),
		new MenuItem({ label: nls.localize('mHideOthers', 'Hide Others'), role: 'hideOthers', accelerator: 'Command+Alt+H' }),
		new MenuItem({ label: nls.localize('mShowAll', 'Show All'), role: 'unhide' }),
		new MenuItem({ type: 'separator' }),
		new MenuItem({ label: nls.localize('miQuit', 'Quit {0}', product.name), role: 'quit', accelerator: 'Command+Q' }),
	]) {
		application.append(item);
	}
	const menu = new Menu();
	menu.append(new MenuItem({ label: product.name, submenu: application }));
	Menu.setApplicationMenu(menu);
}

/**
 * macOS delivers editing and window shortcuts through menu key equivalents.
 * Without an Edit menu they are handled here; menu items, even hidden ones,
 * would make AppKit add Writing Tools and AutoFill to the application menu.
 * While `remoteHasKeyboard` says a remote desktop has the keyboard, they go
 * to the page, which sends them to the other computer.
 */
export function registerEditingShortcuts(window: BrowserWindow, remoteHasKeyboard: () => boolean): void {
	if (!isMacintosh) {
		return;
	}
	const contents = window.webContents;
	contents.on('before-input-event', (event, input) => {
		if (input.type !== 'keyDown' || !input.meta || input.control || input.alt || remoteHasKeyboard()) {
			return;
		}
		switch ((input.shift ? 'shift+' : '') + input.key.toLowerCase()) {
			case 'z': contents.undo(); break;
			case 'shift+z': contents.redo(); break;
			case 'x': contents.cut(); break;
			case 'c': contents.copy(); break;
			case 'v': contents.paste(); break;
			case 'a': contents.selectAll(); break;
			case 'm': window.minimize(); break;
			default: return;
		}
		event.preventDefault();
	});
}

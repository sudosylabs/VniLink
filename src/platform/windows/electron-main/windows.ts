/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import { nativeTheme } from 'electron';
import type { BrowserWindowConstructorOptions } from 'electron';
import { release } from 'node:os';
import { getMacOSWindowControlsPosition, getNativeTitlebarHeight } from '../../window/common/window.js';
export function defaultBrowserWindowOptions(): BrowserWindowConstructorOptions {
	const dark = nativeTheme.shouldUseDarkColors;
	const height = getNativeTitlebarHeight(process.platform, release());
	const options: BrowserWindowConstructorOptions = {
		title: 'VniLink',
		width: 1120,
		height: 760,
		minWidth: 760,
		minHeight: 520,
		show: false,
		backgroundColor: dark ? '#202124' : '#fafafa',
		titleBarStyle: 'hidden',
		webPreferences: {
			sandbox: true,
			contextIsolation: true,
			nodeIntegration: false,
			webviewTag: false,
			spellcheck: false,
			autoplayPolicy: 'user-gesture-required',
		},
	};
	if (process.platform === 'darwin') {
		options.acceptFirstMouse = true;
		options.titleBarOverlay = true;
		options.trafficLightPosition = getMacOSWindowControlsPosition(height, release()) ?? undefined;
	} else {
		options.frame = false;
		options.titleBarOverlay = {
			height: height - 1,
			color: dark ? '#2c2d31' : '#ececef',
			symbolColor: dark ? '#e6e6e8' : '#242426',
		};
	}
	return options;
}

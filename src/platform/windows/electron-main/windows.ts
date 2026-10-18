/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import { nativeTheme } from 'electron';
import type { BrowserWindowConstructorOptions } from 'electron';
import { release } from 'node:os';
import { isMacintosh } from '../../../base/common/platform.js';
import { getMacOSWindowControlsPosition } from '../../window/common/window.js';
import { getWorkbenchTitlebarHeight } from '../../window/common/titlebar.js';
import { resolveWindowMaterial, windowPlatform } from '../../window/common/windowMaterial.js';

export function defaultBrowserWindowOptions(): BrowserWindowConstructorOptions {
	const dark = nativeTheme.shouldUseDarkColors;
	const material = resolveWindowMaterial(windowPlatform, release(), nativeTheme.prefersReducedTransparency, nativeTheme.shouldUseHighContrastColors);
	const height = getWorkbenchTitlebarHeight(release());
	const options: BrowserWindowConstructorOptions = {
		title: 'VniLink',
		width: 1120,
		height: 760,
		minWidth: 760,
		minHeight: 520,
		show: false,
		backgroundColor: material === 'none' ? (dark ? '#202124' : '#fafafa') : '#00000000',
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
	if (isMacintosh) {
		options.acceptFirstMouse = true;
		// Liquid Glass shows through only in a non-opaque window; an opaque one draws its own
		// background behind the glass. Decided by OS support, not the current accessibility
		// settings, so turning Reduce Transparency off later can restore glass in this window.
		if (resolveWindowMaterial(windowPlatform, release(), false, false) === 'glass') {
			options.transparent = true;
		}
		if (material === 'vibrancy') {
			options.vibrancy = 'sidebar';
			options.visualEffectState = 'followWindow';
		}
		options.titleBarOverlay = true;
		options.trafficLightPosition = getMacOSWindowControlsPosition(height, release()) ?? undefined;
	} else {
		options.frame = false;
		if (material === 'mica') {
			options.backgroundMaterial = 'mica';
		}
		options.titleBarOverlay = {
			height: height - 1,
			color: material === 'mica' ? '#00000000' : (dark ? '#2c2d31' : '#ececef'),
			symbolColor: dark ? '#e6e6e8' : '#242426',
		};
	}
	return options;
}

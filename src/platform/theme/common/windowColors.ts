// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { windowPlatform } from '../../window/common/windowMaterial.js';

/**
 * The palette colours the main process paints itself, before the workbench
 * has loaded or behind it: the windows' backgrounds, the Liquid Glass tint
 * and the first colours of the Windows caption buttons. The palette's owner
 * is `colors.css`; each field names the role it copies, and
 * `windowColors.test.ts` keeps them equal. Like VS Code's main-process theme
 * defaults, they exist because a window is painted before any stylesheet can
 * be read.
 */
export interface WindowColors {
	/** `--canvas`: the opaque window background, and every page's. */
	readonly background: string;
	/** `--surface-raised`: the background of the notice window, a raised surface of its own. */
	readonly raisedBackground: string;
	/** `--glass-tint`: the tint Liquid Glass carries; `undefined` where the role is `transparent`. */
	readonly glassTint: string | undefined;
	/** `--opaque-chrome`: the Windows caption buttons' background until the workbench sends its own. */
	readonly titlebar: string;
	/** `--text`: their symbols until then. */
	readonly titlebarForeground: string;
}

/** The pages and the glass, the same on every platform. */
const shared = {
	light: { background: '#ffffff', raisedBackground: '#ffffff', glassTint: '#e8e8ecb3', titlebarForeground: '#242426' },
	dark: { background: '#212123', raisedBackground: '#29292c', glassTint: undefined, titlebarForeground: '#e6e6e8' },
};

/**
 * Solid chrome, where the window has no material, in each platform's own
 * colours: macOS's window chrome, Windows' base layer, libadwaita's header bar.
 */
const chrome: Record<string, { readonly light: string; readonly dark: string }> = {
	darwin: { light: '#ececef', dark: '#2d2d30' },
	win32: { light: '#f3f3f3', dark: '#2b2b2b' },
	linux: { light: '#ebebed', dark: '#2e2e32' },
};

export function getWindowColors(useDarkColors: boolean, platform: string = windowPlatform): WindowColors {
	const scheme = useDarkColors ? 'dark' : 'light';
	return { ...shared[scheme], titlebar: (chrome[platform] ?? chrome.linux)[scheme] };
}

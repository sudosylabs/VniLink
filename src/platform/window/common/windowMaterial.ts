// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { isMacintosh, isWindows } from '../../../base/common/platform.js';

export const windowPlatform = isMacintosh ? 'darwin' : isWindows ? 'win32' : 'linux';

/** `glass` is macOS Liquid Glass (macOS 26 and later); `vibrancy` is its fallback. */
export type WindowBackgroundMaterial = 'none' | 'vibrancy' | 'glass' | 'mica';

export function isWindowBackgroundMaterial(value: unknown): value is WindowBackgroundMaterial {
	return value === 'none' || value === 'vibrancy' || value === 'glass' || value === 'mica';
}

/**
 * A rounded rectangle of the page that the window's material shows through,
 * such as Home's sidebar panel, in CSS pixels from the page's top-left. The
 * page leaves it transparent; on Liquid Glass it gets glass of its own, with
 * edges, while vibrancy and Mica show through by themselves.
 */
export interface IMaterialPanel {
	readonly x: number;
	readonly y: number;
	readonly width: number;
	readonly height: number;
	readonly cornerRadius: number;
}

/** Larger than any window, so a panel within one always passes. */
const MAX_PANEL_EXTENT = 32768;

const MAX_PANEL_RADIUS = 64;

const panelKeys = ['x', 'y', 'width', 'height', 'cornerRadius'];

export function isMaterialPanel(value: unknown): value is IMaterialPanel {
	if (typeof value !== 'object' || value === null || Array.isArray(value)) {
		return false;
	}
	const keys = Object.keys(value);
	if (keys.length !== panelKeys.length || keys.some(key => !panelKeys.includes(key))) {
		return false;
	}
	const extent = (key: string, max: number): boolean => {
		const field: unknown = Reflect.get(value, key);
		return typeof field === 'number' && Number.isFinite(field) && field >= 0 && field <= max;
	};
	return extent('x', MAX_PANEL_EXTENT) && extent('y', MAX_PANEL_EXTENT) && extent('width', MAX_PANEL_EXTENT)
		&& extent('height', MAX_PANEL_EXTENT) && extent('cornerRadius', MAX_PANEL_RADIUS);
}

/** Native backdrops are optional; accessibility preferences take precedence. */
export function resolveWindowMaterial(platform: string, osRelease: string, reducedTransparency: boolean, highContrast: boolean): WindowBackgroundMaterial {
	if (reducedTransparency || highContrast) {
		return 'none';
	}
	const version = /^(\d+)\.(\d+)\.(\d+)/.exec(osRelease);
	if (!version) {
		return 'none';
	}
	const major = Number(version[1]);
	// Darwin 25 is macOS 26, the first with Liquid Glass.
	if (platform === 'darwin' && major >= 25) {
		return 'glass';
	}
	if (platform === 'darwin' && major >= 18) {
		return 'vibrancy';
	}
	if (platform === 'win32' && (major > 10 || major === 10 && Number(version[3]) >= 22621)) {
		return 'mica';
	}
	return 'none';
}

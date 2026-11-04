// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { createRequire } from 'node:module';
import { join } from 'node:path';
import { isMacintosh } from '../../../base/common/platform.js';

/** The window edges a framed view keeps its distance to as the window resizes. */
export interface GlassAnchors {
	readonly top?: boolean;
	readonly right?: boolean;
	readonly bottom?: boolean;
	readonly left?: boolean;
}

/** Where a view goes, in points from the top-left of the window's content. */
export interface GlassFrame {
	readonly x: number;
	readonly y: number;
	readonly width: number;
	readonly height: number;
	readonly anchors?: GlassAnchors;
}

/** Presentation options for one glass attachment; see @vnilink/elg. */
export interface GlassOptions {
	readonly cornerRadius?: number;
	readonly tintColor?: string;
	/** The window's content material behind the glass, as AppKit puts under a sidebar. */
	readonly opaque?: boolean;
	/** Omitted, the glass covers the whole window. */
	readonly frame?: GlassFrame;
}

/**
 * The part of @vnilink/elg's `LiquidGlass` owner the app uses. Declared here
 * because the package installs only on macOS, so other platforms cannot
 * import its types.
 */
export interface LiquidGlassOwner {
	isGlassSupported(): boolean;
	/** A whole-window view is one per window, so adding it again updates it; each framed view is new, above the earlier ones. */
	addView(handle: Buffer, options?: GlassOptions): number;
	/** False when the view is gone or would change between framed and whole-window. */
	updateView(id: number, options?: GlassOptions): boolean;
	removeView(id: number): boolean;
	dispose(): void;
}

interface GlassRuntime {
	readonly platform: string;
	readonly pointerSize: number;
	load(): unknown;
}

interface GlassModule {
	readonly LiquidGlass: new (runtime: GlassRuntime) => LiquidGlassOwner;
}

interface GlassAddon {
	readonly LiquidGlassNative: new () => unknown;
}

/**
 * A new owner for one window, or undefined where Liquid Glass cannot exist.
 * The module and addon are loaded by path, like vnilink.node: the packaged
 * app ships no node_modules. The addon itself loads on first use.
 */
export function createLiquidGlass(): LiquidGlassOwner | undefined {
	if (!isMacintosh) {
		return undefined;
	}
	try {
		// Staged by the build next to the engine addon (build/lib/native.ts `GLASS_DIRECTORY`).
		const glassDirectory = join(__dirname, 'native', 'elg');
		const load = createRequire(__filename);
		const { LiquidGlass } = load(join(glassDirectory, 'index.cjs')) as GlassModule;
		return new LiquidGlass({
			platform: 'darwin',
			pointerSize: 8,
			load: () => new (load(join(glassDirectory, 'elg.node')) as GlassAddon).LiquidGlassNative(),
		});
	} catch {
		// Not staged (a build without the optional package): windows keep vibrancy.
		return undefined;
	}
}

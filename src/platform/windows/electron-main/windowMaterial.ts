// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { nativeTheme } from 'electron';
import type { BrowserWindow, NativeTheme } from 'electron';
import { release } from 'node:os';
import { Disposable } from '../../../base/common/lifecycle.js';
import { Emitter, Event } from '../../../base/common/event.js';
import { getWindowColors } from '../../theme/common/windowColors.js';
import { resolveWindowMaterial, windowPlatform, type IMaterialPanel, type WindowBackgroundMaterial } from '../../window/common/windowMaterial.js';
import { createLiquidGlass, type GlassOptions, type LiquidGlassOwner } from './liquidGlass.js';

/**
 * Glass refracts the window's content material, as AppKit's own sidebar glass
 * does, rather than the desktop: it stays light or dark with the window over
 * any wallpaper. Light glass on that backing is as white as the page, so it
 * carries the palette's light grey, as the system's sidebars are; dark glass
 * reads apart from the page untinted.
 */
function windowGlass(dark: boolean): GlassOptions {
	return { opaque: true, tintColor: getWindowColors(dark).glassTint };
}

/** A panel's glass, on the same backing. It follows the window's height; the page sends a new frame when its width or place changes. */
function panelGlass(panel: IMaterialPanel, dark: boolean): GlassOptions {
	return {
		...windowGlass(dark),
		cornerRadius: panel.cornerRadius,
		frame: { x: panel.x, y: panel.y, width: panel.width, height: panel.height, anchors: { top: true, left: true, bottom: true } },
	};
}

type MaterialWindow = Pick<BrowserWindow, 'isDestroyed' | 'setVibrancy' | 'setBackgroundMaterial' | 'setBackgroundColor' | 'getNativeWindowHandle'>;

/** The theme and accessibility preferences a backdrop follows; Electron's `nativeTheme`, which emits `updated`. */
type MaterialTheme = Event.NodeEventEmitter & Pick<NativeTheme, 'shouldUseDarkColors' | 'prefersReducedTransparency' | 'shouldUseHighContrastColors'>;

/**
 * Owns the native backdrop and its theme/accessibility subscription for one
 * window. On macOS 26 and later that is Liquid Glass behind the web content,
 * falling back to sidebar vibrancy when glass cannot attach, plus glass of its
 * own under the page's material panel.
 */
export class WindowMaterial extends Disposable {
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChange = this.changed.event;
	public material: WindowBackgroundMaterial = 'none';
	/** One owner per window, kept for the window's life: a collected owner removes its glass. */
	private glass: LiquidGlassOwner | undefined;
	private glassView = -1;
	private glassCreated = false;
	/** The page's material panel, in the window's points. */
	private panel: IMaterialPanel | undefined;
	private panelView = -1;

	constructor(
		private readonly window: MaterialWindow,
		private readonly platform: string = windowPlatform,
		private readonly osRelease = release(),
		private readonly createGlass: () => LiquidGlassOwner | undefined = createLiquidGlass,
		private readonly theme: MaterialTheme = nativeTheme,
	) {
		super();
		this._register({
			dispose: () => {
				this.glass?.dispose();
				this.glass = undefined;
			},
		});
		this.update();
		this._register(Event.fromNodeEventEmitter<unknown>(theme, 'updated')(() => {
			this.update();
			this.changed.fire();
		}));
	}

	/** The page's material panel, in the window's points; `undefined` when none is shown. */
	public setPanel(panel: IMaterialPanel | undefined): void {
		this.panel = panel;
		if (!this.window.isDestroyed()) {
			this.updatePanel();
		}
	}

	private update(): void {
		if (this.window.isDestroyed()) {
			return;
		}
		let material = resolveWindowMaterial(this.platform, this.osRelease, this.theme.prefersReducedTransparency, this.theme.shouldUseHighContrastColors);
		const fallback = getWindowColors(this.theme.shouldUseDarkColors, this.platform).background;
		try {
			if (material === 'glass' && !this.attachGlass()) {
				material = 'vibrancy';
			} else if (material !== 'glass') {
				this.detachGlass();
			}
			if (this.platform === 'darwin') {
				this.window.setVibrancy(material === 'vibrancy' ? 'sidebar' : null);
			} else if (resolveWindowMaterial(this.platform, this.osRelease, false, false) === 'mica') {
				this.window.setBackgroundMaterial(material === 'mica' ? 'mica' : 'none');
			}
			this.window.setBackgroundColor(material === 'none' ? fallback : '#00000000');
			this.material = material;
		} catch {
			// A supported OS can still refuse composition. Keep the renderer opaque.
			this.detachGlass();
			this.material = 'none';
			this.window.setBackgroundColor(fallback);
		}
		this.updatePanel();
	}

	/**
	 * Attaches glass, or updates the attachment. The system removes glass when
	 * Reduce Transparency or Increase Contrast turns on and does not restore
	 * it, so every theme update attaches again.
	 */
	private attachGlass(): boolean {
		if (!this.glassCreated) {
			this.glassCreated = true;
			this.glass = this.createGlass();
		}
		if (!this.glass?.isGlassSupported()) {
			return false;
		}
		const view = this.glass.addView(this.window.getNativeWindowHandle(), windowGlass(this.theme.shouldUseDarkColors));
		if (view !== this.glassView) {
			// A new window view is above the panel's, which must be added again to show.
			this.detachPanel();
		}
		this.glassView = view;
		return view >= 0;
	}

	private detachGlass(): void {
		this.detachPanel();
		if (this.glassView >= 0) {
			this.glass?.removeView(this.glassView);
			this.glassView = -1;
		}
	}

	/** Glass under the panel exists only with glass under the window, and moves with the panel. */
	private updatePanel(): void {
		if (this.material !== 'glass' || !this.glass || !this.panel) {
			this.detachPanel();
			return;
		}
		const options = panelGlass(this.panel, this.theme.shouldUseDarkColors);
		if (this.panelView >= 0 && this.glass.updateView(this.panelView, options)) {
			return;
		}
		this.panelView = this.glass.addView(this.window.getNativeWindowHandle(), options);
	}

	private detachPanel(): void {
		if (this.panelView >= 0) {
			this.glass?.removeView(this.panelView);
			this.panelView = -1;
		}
	}
}

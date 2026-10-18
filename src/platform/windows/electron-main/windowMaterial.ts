import { nativeTheme } from 'electron';
import type { BrowserWindow } from 'electron';
import { release } from 'node:os';
import { Disposable } from '../../../base/common/lifecycle.js';
import { Emitter, Event } from '../../../base/common/event.js';
import { resolveWindowMaterial, windowPlatform, type WindowBackgroundMaterial } from '../../window/common/windowMaterial.js';
import { createLiquidGlass, type GlassOptions, type LiquidGlassOwner } from './liquidGlass.js';

/**
 * Light text needs a light surface: glass takes on whatever is behind the window,
 * so in the light theme it carries the same near-white tint vibrancy used
 * (`--native-chrome`), keeping labels readable over a dark desktop.
 */
function glassOptions(): GlassOptions {
	return nativeTheme.shouldUseDarkColors ? {} : { tintColor: '#F8F8FA94' };
}

type MaterialWindow = Pick<BrowserWindow, 'isDestroyed' | 'setVibrancy' | 'setBackgroundMaterial' | 'setBackgroundColor' | 'getNativeWindowHandle'>;

/**
 * Owns the native backdrop and its theme/accessibility subscription for one
 * window. On macOS 26 and later that is Liquid Glass behind the web content,
 * falling back to sidebar vibrancy when glass cannot attach.
 */
export class WindowMaterial extends Disposable {
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChange = this.changed.event;
	public material: WindowBackgroundMaterial = 'none';
	/** One owner per window, kept for the window's life: a collected owner removes its glass. */
	private glass: LiquidGlassOwner | undefined;
	private glassView = -1;
	private glassCreated = false;

	constructor(
		private readonly window: MaterialWindow,
		private readonly platform: string = windowPlatform,
		private readonly osRelease = release(),
		private readonly createGlass: () => LiquidGlassOwner | undefined = createLiquidGlass,
	) {
		super();
		this._register({
			dispose: () => {
				this.glass?.dispose();
				this.glass = undefined;
			},
		});
		this.update();
		this._register(Event.fromNodeEventEmitter<unknown>(nativeTheme, 'updated')(() => {
			this.update();
			this.changed.fire();
		}));
	}

	private update(): void {
		if (this.window.isDestroyed()) {
			return;
		}
		let material = resolveWindowMaterial(this.platform, this.osRelease, nativeTheme.prefersReducedTransparency, nativeTheme.shouldUseHighContrastColors);
		const fallback = nativeTheme.shouldUseDarkColors ? '#202124' : '#fafafa';
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
		this.glassView = this.glass.addView(this.window.getNativeWindowHandle(), glassOptions());
		return this.glassView >= 0;
	}

	private detachGlass(): void {
		if (this.glassView >= 0) {
			this.glass?.removeView(this.glassView);
			this.glassView = -1;
		}
	}
}

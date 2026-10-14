import { nativeTheme } from 'electron';
import type { BrowserWindow } from 'electron';
import { release } from 'node:os';
import { Disposable } from '../../../base/common/lifecycle.js';
import { Emitter, Event } from '../../../base/common/event.js';
import { resolveWindowMaterial, windowPlatform, type WindowBackgroundMaterial } from '../../window/common/windowMaterial.js';

/** Owns the native backdrop and its theme/accessibility subscription for one window. */
export class WindowMaterial extends Disposable {
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChange = this.changed.event;
	public material: WindowBackgroundMaterial = 'none';
	private enabled = true;

	constructor(private readonly window: BrowserWindow, private readonly platform: string = windowPlatform, private readonly osRelease = release()) {
		super();
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
		const material = this.enabled ? resolveWindowMaterial(this.platform, this.osRelease, nativeTheme.prefersReducedTransparency, nativeTheme.shouldUseHighContrastColors) : 'none';
		const fallback = nativeTheme.shouldUseDarkColors ? '#202124' : '#fafafa';
		try {
			if (this.platform === 'darwin') {
				this.window.setVibrancy(material === 'vibrancy' ? 'sidebar' : null);
			} else if (resolveWindowMaterial(this.platform, this.osRelease, false, false) === 'mica') {
				this.window.setBackgroundMaterial(material === 'mica' ? 'mica' : 'none');
			}
			this.window.setBackgroundColor(material === 'none' ? fallback : '#00000000');
			this.material = material;
		} catch {
			// A supported OS can still refuse composition. Keep the renderer opaque.
			this.material = 'none';
			this.window.setBackgroundColor(fallback);
		}
	}
	public setEnabled(enabled: boolean): void {
		if (this.enabled === enabled || this._store.isDisposed) {
			return;
		}
		this.enabled = enabled;
		this.update();
		this.changed.fire();
	}
}

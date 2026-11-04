// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { addDisposableListener, EventType, getWindow, scheduleAtNextAnimationFrame } from '../../../base/browser/dom.js';
import { onUnexpectedError } from '../../../base/common/errors.js';
import { Disposable, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import { INativeHostService } from '../../../platform/native/common/native.js';
import type { IMaterialPanel } from '../../../platform/window/common/windowMaterial.js';

/**
 * Tells the window where a panel that shows its material is, so Liquid Glass
 * can put glass of its own there; other materials show through by
 * themselves. Reports once per frame at most, only when the panel moved,
 * resized or was hidden, and withdraws the panel when disposed.
 */
export class MaterialPanel extends Disposable {
	private readonly frame = this._register(new MutableDisposable());
	/** What the window was last told, as `undefined` or the panel's numbers. */
	private reported: string | undefined = 'none';
	/** The main process turns pixels into points by the zoom, so a new zoom is reported again. */
	private zoomFactor: number | undefined;

	constructor(
		private readonly element: HTMLElement,
		@INativeHostService private readonly nativeHostService: INativeHostService,
	) {
		super();
		const targetWindow = getWindow(element);
		// Its size changes with the window's height, on fullscreen and when its tab is hidden.
		const observer = new targetWindow.ResizeObserver(() => {
			this.schedule();
		});
		observer.observe(element);
		this._register(toDisposable(() => {
			observer.disconnect();
		}));
		// Its place changes with the window's width only through its page, and zoom changes points, not pixels.
		this._register(addDisposableListener(targetWindow, EventType.RESIZE, () => {
			this.schedule();
		}));
		this._register(nativeHostService.onDidChangeWindowState(({ zoomFactor }) => {
			if (zoomFactor !== this.zoomFactor) {
				this.zoomFactor = zoomFactor;
				this.reported = 'none';
				this.schedule();
			}
		}));
		this._register(toDisposable(() => {
			this.report(undefined);
		}));
		this.schedule();
	}

	private schedule(): void {
		if (this.frame.value) {
			return;
		}
		this.frame.value = scheduleAtNextAnimationFrame(getWindow(this.element), () => {
			this.frame.clear();
			this.report(this.measure());
		});
	}

	private measure(): IMaterialPanel | undefined {
		if (!this.element.isConnected) {
			return undefined;
		}
		const { x, y, width, height } = this.element.getBoundingClientRect();
		if (width <= 0 || height <= 0) {
			return undefined;
		}
		const cornerRadius = Number.parseFloat(getWindow(this.element).getComputedStyle(this.element).borderTopLeftRadius) || 0;
		return { x: Math.max(x, 0), y: Math.max(y, 0), width, height, cornerRadius };
	}

	private report(panel: IMaterialPanel | undefined): void {
		const key = panel && [panel.x, panel.y, panel.width, panel.height, panel.cornerRadius].map(value => value.toFixed(2)).join();
		if (key === this.reported) {
			return;
		}
		this.reported = key;
		this.nativeHostService.setMaterialPanel(panel).catch(onUnexpectedError);
	}
}

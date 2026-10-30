/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/workbench/contrib/terminal/browser/terminalResizeDebouncer.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import type { Terminal } from '@xterm/xterm';
import { getWindow, runWhenWindowIdle } from '../../../base/browser/dom.js';
import { RunOnceScheduler } from '../../../base/common/async.js';
import { Disposable, MutableDisposable } from '../../../base/common/lifecycle.js';

/** The normal buffer length at which resizing starts being debounced. */
const startDebouncingThreshold = 200;

const debounceResizeXDelay = 100;

/**
 * VS Code's `TerminalResizeDebouncer` (contrib/terminal/browser), over xterm's
 * own `Terminal`: rows resize at once, columns, which reflow the buffer, wait.
 */
export class TerminalResizeDebouncer extends Disposable {
	private latestX = 0;
	private latestY = 0;
	private readonly resizeXJob = this._register(new MutableDisposable());
	private readonly resizeYJob = this._register(new MutableDisposable());
	private readonly debounceResizeXScheduler = this._register(new RunOnceScheduler(() => {
		this.resizeXCallback(this.latestX);
	}, debounceResizeXDelay));

	constructor(
		private readonly isVisible: () => boolean,
		private readonly getXterm: () => Terminal,
		private readonly resizeBothCallback: (cols: number, rows: number) => void,
		private readonly resizeXCallback: (cols: number) => void,
		private readonly resizeYCallback: (rows: number) => void,
	) {
		super();
	}

	public resize(cols: number, rows: number, immediate: boolean): void {
		if (this._store.isDisposed) {
			return;
		}
		this.latestX = cols;
		this.latestY = rows;
		const xterm = this.getXterm();
		// Resize immediately if requested explicitly or if the buffer is small.
		if (immediate || xterm.buffer.normal.length < startDebouncingThreshold) {
			this.resizeXJob.clear();
			this.resizeYJob.clear();
			this.debounceResizeXScheduler.cancel();
			this.resizeBothCallback(cols, rows);
			return;
		}
		// Resize in an idle callback if the terminal is not visible.
		const win = xterm.element ? getWindow(xterm.element) : undefined;
		if (win && !this.isVisible()) {
			this.resizeXJob.value ??= runWhenWindowIdle(win, () => {
				if (!this._store.isDisposed) {
					this.resizeXCallback(this.latestX);
					this.resizeXJob.clear();
				}
			});
			this.resizeYJob.value ??= runWhenWindowIdle(win, () => {
				if (!this._store.isDisposed) {
					this.resizeYCallback(this.latestY);
					this.resizeYJob.clear();
				}
			});
			return;
		}
		// Vertical resize is cheap and horizontal resize is expensive due to reflow.
		this.resizeYCallback(rows);
		this.debounceResizeXScheduler.schedule();
	}

	public flush(): void {
		if (this._store.isDisposed) {
			return;
		}
		if (this.resizeXJob.value || this.resizeYJob.value || this.debounceResizeXScheduler.isScheduled()) {
			this.resizeXJob.clear();
			this.resizeYJob.clear();
			this.debounceResizeXScheduler.cancel();
			this.resizeBothCallback(this.latestX, this.latestY);
		}
	}
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { addDisposableListener } from '../../../base/browser/dom.js';
import { RunOnceScheduler } from '../../../base/common/async.js';
import type { Event } from '../../../base/common/event.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { SettingsRefresh } from './settingsLayout.js';

/** How often a visible page reads state that has no change event. */
const pollInterval = 2000;

/**
 * Operating-system and engine state has no universal change event. A visible
 * page reads it again when it is shown and when the window gains focus; with
 * `SettingsRefresh.Interval` it also reads it every few seconds. Hidden pages
 * do no background work.
 */
export class SettingsStateObserver extends Disposable {
	private readonly scheduler: RunOnceScheduler | undefined;
	private pending = false;
	private shown = false;

	constructor(private readonly element: HTMLElement, mode: SettingsRefresh, focused: Event<void>, private readonly refresh: () => Promise<void>) {
		super();
		if (mode === SettingsRefresh.Interval) {
			this.scheduler = this._register(new RunOnceScheduler(() => {
				void this.check();
			}, pollInterval));
			this.scheduler.schedule();
		}
		this._register(focused(() => {
			void this.check();
		}));
		this._register(addDisposableListener(element.ownerDocument, 'visibilitychange', () => {
			void this.check();
		}));
		// Home attaches a page's element when it is shown; it then gains a size.
		const resize = new ResizeObserver(() => {
			const shown = this.isShown();
			if (shown && !this.shown) {
				void this.check();
			}
			this.shown = shown;
		});
		resize.observe(element);
		this._register(toDisposable(() => {
			resize.disconnect();
		}));
	}

	private isShown(): boolean {
		return this.element.isConnected && this.element.getClientRects().length > 0 && this.element.ownerDocument.visibilityState !== 'hidden';
	}

	private async check(): Promise<void> {
		if (this.isDisposed() || this.pending) {
			return;
		}
		this.scheduler?.cancel();
		this.pending = true;
		try {
			if (this.isShown()) {
				await this.refresh();
			}
		} finally {
			this.pending = false;
			if (!this.isDisposed()) {
				this.scheduler?.schedule();
			}
		}
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}

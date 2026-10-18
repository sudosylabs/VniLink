import { addDisposableListener } from '../../../base/browser/dom.js';
import { RunOnceScheduler } from '../../../base/common/async.js';
import type { Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';

/** How often a visible page reads state that has no change event. */
const pollInterval = 2000;

/**
 * Operating-system and engine state has no universal change event. Visible
 * pages query it serially and again when the window gains focus; hidden pages
 * do no background work.
 */
export class SettingsStateObserver extends Disposable {
	private readonly scheduler = this._register(new RunOnceScheduler(() => {
		void this.check();
	}, pollInterval));
	private pending = false;

	constructor(private readonly element: HTMLElement, focused: Event<void>, private readonly refresh: () => Promise<void>) {
		super();
		this._register(focused(() => {
			void this.check();
		}));
		this._register(addDisposableListener(element.ownerDocument, 'visibilitychange', () => {
			void this.check();
		}));
		this.scheduler.schedule();
	}

	private async check(): Promise<void> {
		if (this.isDisposed() || this.pending) {
			return;
		}
		this.scheduler.cancel();
		this.pending = true;
		try {
			if (this.element.isConnected && this.element.getClientRects().length > 0 && this.element.ownerDocument.visibilityState !== 'hidden') {
				await this.refresh();
			}
		} finally {
			this.pending = false;
			if (!this.isDisposed()) {
				this.scheduler.schedule();
			}
		}
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}

import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { RunOnceScheduler } from '../../../base/common/async.js';
import type { Event } from '../../../base/common/event.js';

/** OS state has no universal change event. Query visible pages serially and
 * recheck on focus; cached, hidden pages do no background backend work. */
export class SettingsStateObserver extends Disposable {
	private readonly scheduler = this._register(new RunOnceScheduler(() => {
		void this.check();
	}, 2000));
	private pending = false;

	constructor(private readonly element: HTMLElement, focused: Event<void>, private readonly refresh: () => Promise<void>) {
		super();
		this._register(focused(() => {
			void this.check();
		}));
		const visible = () => {
			void this.check();
		};
		document.addEventListener('visibilitychange', visible);
		this._register(toDisposable(() => {
			document.removeEventListener('visibilitychange', visible);
		}));
		this.scheduler.schedule();
	}

	protected disposed(): boolean {
		return this._store.isDisposed;
	}

	private async check(): Promise<void> {
		if (this.disposed() || this.pending) {
			return;
		}
		this.scheduler.cancel();
		this.pending = true;
		try {
			if (this.element.isConnected && this.element.getClientRects().length > 0 && document.visibilityState !== 'hidden') {
				await this.refresh();
			}
		} finally {
			this.pending = false;
			if (!this.disposed()) {
				this.scheduler.schedule();
			}
		}
	}
}

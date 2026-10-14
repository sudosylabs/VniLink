import { Disposable } from '../../../../base/common/lifecycle.js';
import { ITabService, type WorkbenchTab } from '../../../services/tabs/common/tabService.js';
export interface WorkbenchView extends WorkbenchTab {
	readonly element: HTMLElement;
}
export class ViewHost extends Disposable {
	private readonly views = new Map<string, WorkbenchView>();
	constructor(
		private readonly container: HTMLElement,
		@ITabService private readonly tabs: ITabService,
	) {
		super();
		container.className = 'view-host';
		this._register(
			tabs.onDidChange(() => {
				this.render();
			}),
		);
	}
	public open(view: WorkbenchView): void {
		const existing = this.views.get(view.id);
		if (existing) {
			if (existing !== view) {
				view.dispose();
			}
			this.tabs.select(existing.id);
			return;
		}
		this.views.set(view.id, view);
		try {
			this.tabs.open(view);
		} catch (error) {
			this.views.delete(view.id);
			throw error;
		}
	}
	private render(): void {
		const ids = new Set(this.tabs.tabs.map((tab) => tab.id));
		for (const [id, view] of this.views) {
			if (!ids.has(id)) {
				view.element.remove();
				this.views.delete(id);
			}
		}
		for (const view of this.views.values()) {
			const active = this.tabs.activeTab === view;
			view.element.hidden = !active;
			view.element.id = `view-${encodeURIComponent(view.id)}`;
			view.element.setAttribute('role', 'tabpanel');
			view.element.setAttribute('aria-labelledby', `tab-${encodeURIComponent(view.id)}`);
			if (view.element.parentElement !== this.container) {
				this.container.append(view.element);
			}
		}
		this.container.classList.toggle('empty', this.tabs.activeTab === undefined);
	}
}

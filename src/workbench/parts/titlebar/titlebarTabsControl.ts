import * as nls from '../../../nls.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { Icons } from '../../../base/common/icons.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { ITabService } from '../../services/tabs/tabService.js';

/** Adapted layout pattern from VS Code's MultiEditorTabsControl: scrolling tabs and a sibling toolbar. */
export class TitlebarTabsControl extends Disposable {
	private readonly scrollable = document.createElement('div');
	private readonly tablist = document.createElement('div');
	private readonly actions = document.createElement('div');
	private dragged: string | undefined;
	private selectedId: string | undefined;

	constructor(
		parent: HTMLElement,
		private readonly onError: (error: unknown) => void,
		@ITabService private readonly service: ITabService,
	) {
		super();
		const row = document.createElement('div');
		row.className = 'tabs-and-actions-container';
		this.scrollable.className = 'tabs-scrollable';
		this.tablist.className = 'tabs-container';
		this.tablist.setAttribute('role', 'tablist');
		this.tablist.setAttribute('aria-label', nls.localize('tabs.openViews', 'Open views'));
		this.actions.className = 'view-actions';
		this.actions.setAttribute('role', 'toolbar');
		this.scrollable.append(this.tablist);
		row.append(this.scrollable, this.actions);
		parent.append(row);
		this._register(
			toDisposable(() => {
				row.remove();
			}),
		);
		this.listen(this.tablist, 'click', (event) => {
			const target = this.target(event);
			if (!target) {
				return;
			}
			if (event.target instanceof Element && event.target.closest('.tab-close')) {
				void service.close(target.dataset.tabId ?? '').catch(onError);
			} else {
				service.select(target.dataset.tabId ?? '');
			}
		});
		this.listen(this.tablist, 'auxclick', (event) => {
			if (event instanceof MouseEvent && event.button === 1) {
				const target = this.target(event);
				if (target) {
					event.preventDefault();
					void service.close(target.dataset.tabId ?? '').catch(onError);
				}
			}
		});
		this.listen(this.tablist, 'keydown', (event) => {
			if (event instanceof KeyboardEvent) {
				this.onKeyDown(event);
			}
		});
		this.listen(this.tablist, 'dragstart', (event) => {
			if (!(event instanceof DragEvent)) {
				return;
			}
			const target = this.target(event);
			this.dragged = target?.dataset.tabId;
			if (this.dragged && event.dataTransfer) {
				event.dataTransfer.effectAllowed = 'move';
				event.dataTransfer.setData('application/x-vnilink-tab', this.dragged);
			}
		});
		this.listen(this.tablist, 'dragover', (event) => {
			if (this.dragged && event instanceof DragEvent) {
				event.preventDefault();
				if (event.dataTransfer) {
					event.dataTransfer.dropEffect = 'move';
				}
			}
		});
		this.listen(this.tablist, 'drop', (event) => {
			const target = this.target(event);
			if (this.dragged && target) {
				event.preventDefault();
				const index = service.tabs.findIndex((tab) => tab.id === target.dataset.tabId);
				service.move(this.dragged, index);
			}
			this.dragged = undefined;
		});
		this.listen(this.tablist, 'dragend', () => {
			this.dragged = undefined;
		});
		const wheel = (event: WheelEvent): void => {
			if (this.scrollable.scrollWidth > this.scrollable.clientWidth && event.deltaY && !event.deltaX) {
				event.preventDefault();
				this.scrollable.scrollLeft += event.deltaY;
			}
		};
		this.scrollable.addEventListener('wheel', wheel, { passive: false });
		this._register(
			toDisposable(() => {
				this.scrollable.removeEventListener('wheel', wheel);
			}),
		);
		this.listen(this.actions, 'click', (event) => {
			if (event.target instanceof Element) {
				const id = event.target.closest<HTMLButtonElement>('button')?.dataset.actionId;
				if (id) {
					void service.executeAction(id).catch(onError);
				}
			}
		});
		this.listen(this.actions, 'keydown', (event) => {
			if (!(event instanceof KeyboardEvent)) {
				return;
			}
			const buttons = Array.from(this.actions.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'));
			const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
			const next =
				event.key === 'ArrowRight'
					? (index + 1) % buttons.length
					: event.key === 'ArrowLeft'
						? (index + buttons.length - 1) % buttons.length
						: event.key === 'Home'
							? 0
							: event.key === 'End'
								? buttons.length - 1
								: undefined;
			if (next !== undefined && buttons[next]) {
				event.preventDefault();
				buttons[next].focus();
			}
		});
		const observer = new ResizeObserver(() => {
			this.revealSelected();
		});
		observer.observe(this.scrollable);
		this._register(
			toDisposable(() => {
				observer.disconnect();
			}),
		);
		this._register(
			service.onDidChange(() => {
				this.render();
			}),
		);
		this.render();
	}

	private listen(target: HTMLElement, type: string, listener: (event: Event) => void): void {
		target.addEventListener(type, listener);
		this._register(
			toDisposable(() => {
				target.removeEventListener(type, listener);
			}),
		);
	}

	private target(event: Event): HTMLElement | null {
		return event.target instanceof Element ? event.target.closest<HTMLElement>('[role="tab"]') : null;
	}

	private onKeyDown(event: KeyboardEvent): void {
		const target = this.target(event);
		if (!target) {
			return;
		}
		const tabs = this.service.tabs;
		const index = tabs.findIndex((tab) => tab.id === target.dataset.tabId);
		if (event.key === 'Delete') {
			event.preventDefault();
			void this.service.close(target.dataset.tabId ?? '').catch(this.onError);
			return;
		}
		const next =
			event.key === 'ArrowRight'
				? (index + 1) % tabs.length
				: event.key === 'ArrowLeft'
					? (index + tabs.length - 1) % tabs.length
					: event.key === 'Home'
						? 0
						: event.key === 'End'
							? tabs.length - 1
							: undefined;
		const tab = next === undefined ? undefined : tabs[next];
		if (tab) {
			event.preventDefault();
			this.service.select(tab.id);
			this.tablist.querySelector<HTMLElement>('[aria-selected="true"]')?.focus();
		}
	}

	private render(): void {
		const focused = document.activeElement instanceof HTMLElement ? document.activeElement : undefined;
		const tabFocus = focused?.closest<HTMLElement>('[role="tab"]')?.dataset.tabId;
		const actionFocus = focused?.dataset.actionId;
		const scrollLeft = this.scrollable.scrollLeft;
		this.tablist.replaceChildren();
		for (const tab of this.service.tabs) {
			const element = document.createElement('div');
			element.className = tab.permanent ? 'tab sizing-fit permanent' : 'tab sizing-fit';
			element.dataset.tabId = tab.id;
			element.id = `tab-${encodeURIComponent(tab.id)}`;
			element.setAttribute('role', 'tab');
			element.setAttribute('aria-controls', `view-${encodeURIComponent(tab.id)}`);
			const active = this.service.activeTab === tab;
			element.setAttribute('aria-selected', String(active));
			element.tabIndex = active ? 0 : -1;
			element.draggable = !tab.permanent;
			const fill = document.createElement('span');
			fill.className = 'tab-fill';
			element.append(fill);
			if (tab.icon) {
				element.append(renderIcon(tab.icon));
			}
			const label = document.createElement('span');
			label.className = 'tab-label';
			label.textContent = tab.label;
			element.title = tab.label;
			element.append(label);
			if (!tab.permanent) {
				const close = document.createElement('button');
				close.type = 'button';
				close.className = 'tab-close';
				close.tabIndex = -1;
				close.setAttribute('aria-label', nls.localize('tabs.close', 'Close {0}', tab.label));
				close.append(renderIcon(Icons.close));
				element.append(close);
			}
			this.tablist.append(element);
		}
		this.actions.replaceChildren();
		const active = this.service.activeTab;
		this.actions.setAttribute('aria-label', active ? nls.localize('tabs.tabActions', '{0} actions', active.label) : nls.localize('tabs.viewActions', 'View actions'));
		for (const action of active?.actions ?? []) {
			const button = document.createElement('button');
			button.type = 'button';
			button.className = 'view-action';
			button.dataset.actionId = action.id;
			button.title = action.label;
			button.setAttribute('aria-label', action.label);
			button.disabled = !active || !this.service.isActionEnabled(active, action);
			if (action.icon) {
				button.append(renderIcon(action.icon));
			} else {
				button.textContent = action.label;
			}
			this.actions.append(button);
		}
		if (tabFocus) {
			const target = Array.from(this.tablist.children).find(
				(element) => element instanceof HTMLElement && element.dataset.tabId === tabFocus,
			);
			if (target instanceof HTMLElement) {
				target.focus();
			} else {
				this.tablist.querySelector<HTMLElement>('[aria-selected="true"]')?.focus();
			}
		}
		if (actionFocus) {
			this.actions.querySelector<HTMLButtonElement>(`[data-action-id="${CSS.escape(actionFocus)}"]`)?.focus();
		}
		this.scrollable.scrollLeft = scrollLeft;
		if (this.selectedId !== active?.id) {
			this.selectedId = active?.id;
			this.revealSelected();
		}
	}

	private revealSelected(): void {
		const selected = this.tablist.querySelector<HTMLElement>('[aria-selected="true"]');
		if (selected) {
			const left = selected.offsetLeft;
			const right = left + selected.offsetWidth;
			if (left < this.scrollable.scrollLeft) {
				this.scrollable.scrollLeft = left;
			} else if (right > this.scrollable.scrollLeft + this.scrollable.clientWidth) {
				this.scrollable.scrollLeft = right - this.scrollable.clientWidth;
			}
		}
	}
}

import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { onUnexpectedError } from '../../../base/common/errors.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { ITabService, type WorkbenchTab } from '../../services/tabs/tabService.js';
import { ViewActionsToolbar } from './viewActionsToolbar.js';

/** Tab ids travel through drag data under this type. */
const tabDragType = 'application/x-vnilink-tab';

/**
 * Scrolling tabs with the active tab's toolbar beside them, following the
 * layout of VS Code's `MultiEditorTabsControl`.
 */
export class TitlebarTabsControl extends Disposable {
	private readonly scrollable: HTMLElement;
	private readonly tablist: HTMLElement;
	private dragged: string | undefined;
	private selectedId: string | undefined;

	constructor(
		parent: HTMLElement,
		@ITabService private readonly tabService: ITabService,
		@IInstantiationService instantiationService: IInstantiationService,
	) {
		super();
		const row = append(parent, $('.tabs-and-actions-container'));
		this._register(toDisposable(() => {
			row.remove();
		}));
		this.scrollable = append(row, $('.tabs-scrollable'));
		this.tablist = append(this.scrollable, $('.tabs-container', { role: 'tablist', 'aria-label': nls.localize('tabs.openViews', 'Open views') }));
		const actions = append(row, $('.view-actions'));
		this._register(instantiationService.createInstance(ViewActionsToolbar, actions));
		this.registerListeners();
		this.render();
	}

	private registerListeners(): void {
		this._register(addDisposableListener(this.tablist, EventType.CLICK, event => {
			const id = this.tabId(event);
			if (id === undefined) {
				return;
			}
			if (event.target instanceof Element && event.target.closest('.tab-close')) {
				this.close(id);
			} else {
				this.tabService.select(id);
			}
		}));
		this._register(addDisposableListener(this.tablist, EventType.AUXCLICK, event => {
			const id = this.tabId(event);
			if (id !== undefined && event.button === 1) {
				event.preventDefault();
				this.close(id);
			}
		}));
		this._register(addDisposableListener(this.tablist, EventType.KEY_DOWN, event => {
			this.onKeyDown(event);
		}));
		this._register(addDisposableListener(this.tablist, EventType.DRAG_START, event => {
			this.dragged = this.tabId(event);
			if (this.dragged && event.dataTransfer) {
				event.dataTransfer.effectAllowed = 'move';
				event.dataTransfer.setData(tabDragType, this.dragged);
			}
		}));
		this._register(addDisposableListener(this.tablist, EventType.DRAG_OVER, event => {
			if (this.dragged) {
				event.preventDefault();
				if (event.dataTransfer) {
					event.dataTransfer.dropEffect = 'move';
				}
			}
		}));
		this._register(addDisposableListener(this.tablist, EventType.DROP, event => {
			const target = this.tabId(event);
			if (this.dragged && target !== undefined) {
				event.preventDefault();
				this.tabService.move(this.dragged, this.tabService.tabs.findIndex(tab => tab.id === target));
			}
			this.dragged = undefined;
		}));
		this._register(addDisposableListener(this.tablist, EventType.DRAG_END, () => {
			this.dragged = undefined;
		}));
		this._register(addDisposableListener(this.scrollable, EventType.MOUSE_WHEEL, (event: WheelEvent) => {
			if (this.scrollable.scrollWidth > this.scrollable.clientWidth && event.deltaY && !event.deltaX) {
				event.preventDefault();
				this.scrollable.scrollLeft += event.deltaY;
			}
		}, { passive: false }));
		const observer = new ResizeObserver(() => {
			this.revealSelected();
		});
		observer.observe(this.scrollable);
		this._register(toDisposable(() => {
			observer.disconnect();
		}));
		this._register(this.tabService.onDidChange(() => {
			this.render();
		}));
	}

	private tabId(event: Event): string | undefined {
		return event.target instanceof Element ? event.target.closest<HTMLElement>('[role="tab"]')?.dataset.tabId : undefined;
	}

	private close(id: string): void {
		this.tabService.close(id).catch(onUnexpectedError);
	}

	private onKeyDown(event: KeyboardEvent): void {
		const id = this.tabId(event);
		if (id === undefined) {
			return;
		}
		if (event.key === 'Delete') {
			event.preventDefault();
			this.close(id);
			return;
		}
		const tabs = this.tabService.tabs;
		const index = tabs.findIndex(tab => tab.id === id);
		const next = neighbor(event.key, index, tabs.length);
		const tab = next === undefined ? undefined : tabs[next];
		if (tab) {
			event.preventDefault();
			this.tabService.select(tab.id);
			this.tablist.querySelector<HTMLElement>('[aria-selected="true"]')?.focus();
		}
	}

	private render(): void {
		const focused = document.activeElement instanceof HTMLElement ? document.activeElement.closest<HTMLElement>('[role="tab"]')?.dataset.tabId : undefined;
		const scrollLeft = this.scrollable.scrollLeft;
		this.tablist.replaceChildren(...this.tabService.tabs.map(tab => this.renderTab(tab)));
		if (focused) {
			const target = [...this.tablist.children].find(element => element instanceof HTMLElement && element.dataset.tabId === focused);
			(target instanceof HTMLElement ? target : this.tablist.querySelector<HTMLElement>('[aria-selected="true"]'))?.focus();
		}
		this.scrollable.scrollLeft = scrollLeft;
		const active = this.tabService.activeTab;
		if (this.selectedId !== active?.id) {
			this.selectedId = active?.id;
			this.revealSelected();
		}
	}

	private renderTab(tab: WorkbenchTab): HTMLElement {
		const active = this.tabService.activeTab === tab;
		const element = $(tab.permanent ? '.tab.sizing-fit.permanent' : '.tab.sizing-fit', {
			id: `tab-${encodeURIComponent(tab.id)}`,
			role: 'tab',
			title: tab.label,
			tabindex: active ? '0' : '-1',
			draggable: tab.permanent ? 'false' : 'true',
			'aria-controls': `view-${encodeURIComponent(tab.id)}`,
			'aria-selected': String(active),
		});
		element.dataset.tabId = tab.id;
		append(element, $('span.tab-fill'));
		if (tab.icon) {
			element.append(renderIcon(tab.icon));
		}
		append(element, $('span.tab-label')).textContent = tab.label;
		if (!tab.permanent) {
			const close = append(element, $<HTMLButtonElement>('button.tab-close', { type: 'button', tabindex: '-1', 'aria-label': nls.localize('tabs.close', 'Close {0}', tab.label) }));
			close.append(renderIcon(Icons.close));
		}
		return element;
	}

	private revealSelected(): void {
		const selected = this.tablist.querySelector<HTMLElement>('[aria-selected="true"]');
		if (!selected) {
			return;
		}
		const left = selected.offsetLeft;
		const right = left + selected.offsetWidth;
		if (left < this.scrollable.scrollLeft) {
			this.scrollable.scrollLeft = left;
		} else if (right > this.scrollable.scrollLeft + this.scrollable.clientWidth) {
			this.scrollable.scrollLeft = right - this.scrollable.clientWidth;
		}
	}
}

/** The tab index an arrow, Home or End key moves to, wrapping at the ends. */
function neighbor(key: string, index: number, count: number): number | undefined {
	switch (key) {
		case 'ArrowRight': return (index + 1) % count;
		case 'ArrowLeft': return (index + count - 1) % count;
		case 'Home': return 0;
		case 'End': return count - 1;
		default: return undefined;
	}
}

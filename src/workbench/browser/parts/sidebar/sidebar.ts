import { Disposable, toDisposable } from '../../../../base/common/lifecycle.js';
import { Emitter } from '../../../../base/common/event.js';
import type { IconId } from '../../../../base/common/icons.js';
import { button } from '../../../../base/browser/ui/button/vnilinkButton.js';
import { renderIcon } from '../../../../base/browser/ui/icon/icon.js';

export interface SidebarItem {
	readonly id: string;
	readonly label: string;
	readonly icon: IconId;
}

export interface SidebarGroup {
	readonly label?: string;
	readonly items: readonly SidebarItem[];
}

/** Navigation belongs to its view; no application services or settings live here. */
export class Sidebar extends Disposable {
	public readonly element = document.createElement('aside');
	private readonly selected = this._register(new Emitter<string>());
	public readonly onDidSelect = this.selected.event;
	private readonly items = new Map<string, { element: HTMLButtonElement; icon: IconId }>();
	private active: string | undefined;

	constructor(label: string, groups: readonly SidebarGroup[], initial: string) {
		super();
		this.element.className = 'sidebar';
		const nav = document.createElement('nav');
		nav.setAttribute('aria-label', label);
		for (const group of groups) {
			const section = document.createElement('div');
			section.className = 'sidebar-section';
			if (group.label) {
				const heading = document.createElement('h2');
				heading.className = 'sidebar-heading';
				heading.textContent = group.label;
				section.append(heading);
			}
			for (const item of group.items) {
				const entry = button(item.label, 'nav-item', { icon: item.icon });
				const select = (): void => {
					this.select(item.id);
				};
				entry.addEventListener('click', select);
				this._register(toDisposable(() => {
					entry.removeEventListener('click', select);
				}));
				entry.dataset.sectionId = item.id;
				this.items.set(item.id, { element: entry, icon: item.icon });
				section.append(entry);
			}
			nav.append(section);
		}
		this.element.append(nav);
		this.select(initial);
		this._register(toDisposable(() => {
			this.element.remove();
			this.items.clear();
		}));
	}

	public select(id: string): void {
		if (!this.items.has(id) || this.active === id) {
			return;
		}
		this.active = id;
		for (const [key, { element: entry, icon }] of this.items) {
			entry.classList.toggle('selected', key === id);
			entry.querySelector('.vnilink-icon')?.replaceWith(renderIcon(icon, { variant: key === id ? 'filled' : 'outlined' }));
			if (key === id) {
				entry.setAttribute('aria-current', 'page');
			} else {
				entry.removeAttribute('aria-current');
			}
		}
		this.selected.fire(id);
	}
}

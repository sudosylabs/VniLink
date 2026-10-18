import * as nls from '../../../nls.js';
import { $, append } from '../../../base/browser/dom.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable, dispose } from '../../../base/common/lifecycle.js';
import { IContextKeyService, type IContextKey } from '../../../platform/contextkey/common/contextkey.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { Registry } from '../../../platform/registry/common/platform.js';
import { HomePageContext } from '../../contextkeys.js';
import type { WorkbenchTab } from '../../services/tabs/tabService.js';
import { Extensions, HomePageGroup, type IHomePage, type IHomePagesRegistry } from './homePages.js';
import { Sidebar, type SidebarGroup } from './sidebar.js';

/**
 * The permanent Home tab: a sidebar of contributed pages beside the page
 * being shown. Pages are created on first visit and kept until Home closes.
 */
export class HomeView extends Disposable implements WorkbenchTab {
	public static readonly ID = 'home';
	public readonly id = HomeView.ID;
	public readonly permanent = true;
	public readonly label = nls.localize('home.viewLabel', 'Home');
	public readonly icon = Icons.home;
	public readonly element: HTMLElement;
	private readonly content: HTMLElement;
	private readonly sidebar: Sidebar;
	private readonly registry = Registry.as<IHomePagesRegistry>(Extensions.HomePages);
	private readonly pages = new Map<string, IHomePage>();
	private activePage: IHomePage | undefined;
	private readonly pageContext: IContextKey<string>;

	constructor(
		@IInstantiationService private readonly instantiationService: IInstantiationService,
		@IContextKeyService contextKeyService: IContextKeyService,
	) {
		super();
		// Pages contribute titlebar actions with `when` clauses on the page shown.
		this.pageContext = HomePageContext.bindTo(contextKeyService);
		this.element = $('.home-view');
		this.sidebar = this._register(new Sidebar(nls.localize('home.navigation', 'Home navigation')));
		this.content = $('.home-content');
		append(this.element, this.sidebar.element, this.content);
		this._register(this.sidebar.onDidSelect(id => {
			this.showPage(id);
		}));
		this._register(this.registry.onDidChangePages(() => {
			this.sidebar.setGroups(this.groups());
		}));
		this.sidebar.setGroups(this.groups());
		const first = this.registry.getPages(HomePageGroup.Devices).at(0) ?? this.registry.getPages(HomePageGroup.Settings).at(0);
		if (first) {
			this.sidebar.select(first.id);
		}
	}

	private groups(): SidebarGroup[] {
		const items = (group: HomePageGroup) => this.registry.getPages(group).map(page => ({ id: page.id, label: page.title, icon: page.icon }));
		return [
			{ items: items(HomePageGroup.Devices) },
			{ label: nls.localize('home.settings', 'Settings'), items: items(HomePageGroup.Settings) },
		];
	}

	private showPage(id: string): void {
		const page = this.getPage(id);
		if (!page || page === this.activePage) {
			return;
		}
		this.activePage = page;
		this.pageContext.set(id);
		this.content.replaceChildren(page.element);
		this.content.scrollTop = 0;
	}

	private getPage(id: string): IHomePage | undefined {
		const existing = this.pages.get(id);
		const descriptor = this.registry.getPage(id);
		if (existing || !descriptor) {
			return existing;
		}
		const page = this.instantiationService.createInstance<IHomePage>(descriptor.ctorDescriptor);
		this.pages.set(id, page);
		return page;
	}

	public override dispose(): void {
		this.pageContext.reset();
		super.dispose();
		dispose(this.pages.values());
		this.pages.clear();
		this.element.remove();
	}
}

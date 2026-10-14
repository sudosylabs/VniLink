import { FilesPage } from '../../settings/browser/filesPage.js';
import { TerminalPage } from '../../settings/browser/terminalPage.js';
import { InputPage } from '../../settings/browser/inputPage.js';
import { IApplicationService } from '../../../services/application/common/applicationService.js';
import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { Emitter } from '../../../../base/common/event.js';
import { Icons } from '../../../../base/common/icons.js';
import { IInstantiationService } from '../../../../platform/instantiation/common/instantiation.js';
import type { TabAction } from '../../../services/tabs/common/tabService.js';
import type { WorkbenchView } from '../../../browser/parts/views/viewHost.js';
import { Sidebar } from '../../../browser/parts/sidebar/sidebar.js';
import { AppearancePage } from '../../settings/browser/appearancePage.js';
import { AccessPage } from '../../settings/browser/accessPage.js';
import { ConnectionPage } from '../../settings/browser/connectionPage.js';
import { RemoteDesktopPage } from '../../settings/browser/remoteDesktopPage.js';
import { DevicesPage } from './devicesPage.js';
import type { WorkbenchPage } from '../../../browser/parts/views/page.js';
import { homeNavigationGroups, isHomeSectionId, type HomeSectionId } from './homeNavigation.js';

/** Owns navigation and page lifetimes; each page owns its content and actions. */
export class HomeView extends Disposable implements WorkbenchView {
	public readonly id = 'home';
	public readonly permanent = true;
	public readonly label = nls.localize('home.viewLabel', 'Home');
	public readonly icon = Icons.home;
	public readonly element = document.createElement('div');
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChangeActions = this.changed.event;
	private readonly content = document.createElement('div');
	private readonly pages = new Map<HomeSectionId, WorkbenchPage>();
	private activePage: WorkbenchPage;

	public get actions(): readonly TabAction[] {
		return this.application.restartRequired ? [{ id: 'home.restart', label: nls.localize('app.restart', 'Restart VniLink'), icon: Icons.restart, run: () => this.application.restart() }, ...this.activePage.actions] : this.activePage.actions;
	}

	constructor(@IInstantiationService private readonly instantiationService: IInstantiationService, @IApplicationService private readonly application: IApplicationService) {
		super();
		this.element.className = 'home-view';
		this._register(application.onDidChangeRestartRequired(() => {
			this.changed.fire();
		}));
		this.content.className = 'home-content';
		this.activePage = this.getPage('devices');
		this.content.append(this.activePage.element);
		const sidebar = this._register(new Sidebar(nls.localize('home.navigation', 'Home navigation'), homeNavigationGroups, 'devices'));
		this.element.append(sidebar.element, this.content);
		this._register(sidebar.onDidSelect(id => {
			if (isHomeSectionId(id)) {
				this.selectSection(id);
			}
		}));
	}

	private getPage(id: HomeSectionId): WorkbenchPage {
		let page = this.pages.get(id);
		if (!page) {
			if (id === 'devices') {
				page = this.instantiationService.createInstance(DevicesPage);
			} else if (id === 'appearance') {
				page = this.instantiationService.createInstance(AppearancePage);
			} else if (id === 'access') {
				page = this.instantiationService.createInstance(AccessPage);
			} else if (id === 'desktop') {
				page = this.instantiationService.createInstance(RemoteDesktopPage);
			} else if (id === 'input') {
				page = this.instantiationService.createInstance(InputPage);
			} else if (id === 'files') {
				page = this.instantiationService.createInstance(FilesPage);
			} else if (id === 'terminal') {
				page = this.instantiationService.createInstance(TerminalPage);
			} else if (id === 'connection') {
				page = this.instantiationService.createInstance(ConnectionPage);
			} else {
				throw new Error(`Settings page has no implementation: ${id}`);
			}
			this.pages.set(id, this._register(page));
		}
		return page;
	}

	private selectSection(id: HomeSectionId): void {
		const page = this.getPage(id);
		if (page === this.activePage) {
			return;
		}
		this.activePage = page;
		this.content.replaceChildren(page.element);
		this.content.scrollTop = 0;
		this.changed.fire();
	}

	public override dispose(): void {
		super.dispose();
		this.pages.clear();
		this.element.remove();
	}
}

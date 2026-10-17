import { $, append } from '../../../base/browser/dom.js';
import { IThemeService } from '../../../platform/theme/common/themeService.js';
import { Part } from '../../part.js';
import { ITabService } from '../../services/tabs/tabService.js';

/**
 * Shows the active tab's view, in the role of VS Code's editor part. Every
 * open tab keeps its element here; only the active one is visible.
 */
export class ViewsPart extends Part {
	private host: HTMLElement | undefined;

	constructor(
		@ITabService private readonly tabService: ITabService,
		@IThemeService themeService: IThemeService,
	) {
		super('workbench.parts.views', themeService);
		this._register(tabService.onDidChange(() => {
			this.render();
		}));
	}

	protected override createContentArea(parent: HTMLElement): HTMLElement {
		parent.classList.add('view-stack');
		this.host = append(parent, $('.view-host'));
		this.render();
		return this.host;
	}

	private render(): void {
		const host = this.host;
		if (!host) {
			return;
		}
		const tabs = this.tabService.tabs;
		for (const child of [...host.children]) {
			if (!tabs.some(tab => tab.element === child)) {
				child.remove();
			}
		}
		for (const tab of tabs) {
			const element = tab.element;
			element.hidden = this.tabService.activeTab !== tab;
			element.id = `view-${encodeURIComponent(tab.id)}`;
			element.setAttribute('role', 'tabpanel');
			element.setAttribute('aria-labelledby', `tab-${encodeURIComponent(tab.id)}`);
			if (element.parentElement !== host) {
				host.append(element);
			}
		}
		host.classList.toggle('empty', this.tabService.activeTab === undefined);
	}
}

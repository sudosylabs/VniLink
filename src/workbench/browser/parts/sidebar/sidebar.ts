import * as nls from '../../../../nls.js';
import { Icons } from '../../../../base/common/icons.js';
import { button } from '../../../../base/browser/ui/button/vnilinkButton.js';

export function createSidebar(onComputers: () => void): HTMLElement {
	const sidebar = document.createElement('aside');
	sidebar.className = 'sidebar';
	const brand = document.createElement('div');
	brand.className = 'brand';
	const logo = document.createElement('img');
	logo.src = 'app-icon.svg'; logo.alt = ''; logo.width = 24; logo.height = 24;
	const name = document.createElement('span'); name.textContent = 'VniLink';
	brand.append(logo, name);
	const nav = document.createElement('nav'); nav.setAttribute('aria-label', nls.localize('sidebar.navLabel', 'Home navigation'));
	const computers = button(nls.localize('sidebar.computers', 'Computers'), 'nav-item selected', { icon: Icons.computers, variant: 'filled', onClick: onComputers }); computers.setAttribute('aria-current', 'page');
	nav.append(computers);
	sidebar.append(brand, nav);
	return sidebar;
}

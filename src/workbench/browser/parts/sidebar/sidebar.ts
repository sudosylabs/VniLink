import { button } from '../../../../base/browser/ui/button/button.js';
export function createSidebar(): HTMLElement {
	const sidebar = document.createElement('aside');
	sidebar.className = 'sidebar';
	const brand = document.createElement('div');
	brand.className = 'brand';
	const logo = document.createElement('img');
	logo.src = 'app-icon.svg'; logo.alt = ''; logo.width = 24; logo.height = 24;
	const name = document.createElement('span'); name.textContent = 'VniLink';
	brand.append(logo, name);
	const nav = document.createElement('nav'); nav.setAttribute('aria-label', 'Home navigation');
	const computers = button('Computers', 'nav-item selected'); computers.setAttribute('aria-current', 'page');
	nav.append(computers);
	sidebar.append(brand, nav);
	return sidebar;
}

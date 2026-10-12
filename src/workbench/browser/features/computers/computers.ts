import { Icons } from '../../../../base/common/icons.js';
import { renderIcon } from '../../../../base/browser/ui/icon/icon.js';
export function createComputersPage(): HTMLElement {
	const page = document.createElement('main');
	// This static markup contains no engine/device data. Future labels use textContent.
	page.innerHTML = `<header class="page-heading"><h1>Your computers</h1><p>A place for the computers you connect to.</p></header>
    <section class="empty-state" aria-label="Computers">
      <h2>No computers yet</h2><p>Paired computers will appear here.</p>
    </section>`;
	const icon = renderIcon(Icons.computers);
	icon.classList.add('computer');
	page.querySelector('.empty-state')?.prepend(icon);
	return page;
}

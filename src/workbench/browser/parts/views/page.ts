import type { IDisposable } from '../../../../base/common/lifecycle.js';
import type { TabAction } from '../../../services/tabs/common/tabService.js';

export interface WorkbenchPage extends IDisposable {
	readonly element: HTMLElement;
	readonly actions: readonly TabAction[];
}

export function createPageHeading(label: string, description: string): HTMLElement {
	const heading = document.createElement('header');
	heading.className = 'page-heading';
	const title = document.createElement('h1');
	title.textContent = label;
	const detail = document.createElement('p');
	detail.textContent = description;
	heading.append(title, detail);
	return heading;
}

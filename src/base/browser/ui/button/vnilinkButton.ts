import type { IconId, IconVariant } from '../../../common/icons.js';
import { renderIcon } from '../icon/icon.js';
export interface ButtonOptions {
	icon?: IconId;
	variant?: IconVariant;
	onClick?: () => void;
}
export function button(label: string, className: string, options: ButtonOptions = {}): HTMLButtonElement {
	const element = document.createElement('button');
	element.type = 'button';
	element.className = className;
	element.textContent = label;
	if (options.icon) {
		element.prepend(renderIcon(options.icon, { variant: options.variant }));
	}
	if (options.onClick) {
		element.addEventListener('click', options.onClick);
	}
	return element;
}

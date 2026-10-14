import './icon.css';
import type { IconId, IconVariant } from '../../../common/icons.js';
import { iconSvg } from './iconAssets.js';

export interface IconOptions {
	variant?: IconVariant;
	/** Omit for decorative icons; name icon-only buttons on the button itself. */
	label?: string;
}
/** Render bundled, trusted SVG artwork. No paths or arbitrary SVG accepted. */
export function renderIcon(icon: IconId, options: IconOptions = {}): HTMLSpanElement {
	const element = document.createElement('span');
	const variant = options.variant ?? 'outlined';
	element.className = 'vnilink-icon';
	element.dataset.icon = icon;
	element.dataset.variant = variant;
	// Artwork is supplied exclusively by the pinned build dependency.
	const template = document.createElement('template');
	template.innerHTML = iconSvg(icon, variant);
	const svg = template.content.firstElementChild;
	if (!(svg instanceof SVGSVGElement)) {
		throw new Error(`Invalid artwork for icon: ${icon}`);
	}
	svg.setAttribute('aria-hidden', 'true');
	svg.setAttribute('focusable', 'false');
	if (options.label?.trim()) {
		element.setAttribute('role', 'img');
		element.setAttribute('aria-label', options.label);
	} else {
		element.setAttribute('aria-hidden', 'true');
	}
	element.append(svg);
	return element;
}

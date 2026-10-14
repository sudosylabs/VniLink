/** Separate a section’s explanation from its controls without changing their ownership. */
export function appendAccessSection(section: HTMLElement, heading: HTMLElement, description: HTMLElement, controls: readonly HTMLElement[]): void {
	const intro = document.createElement('div');
	intro.className = 'access-section-intro';
	intro.append(heading, description);
	const body = document.createElement('div');
	body.className = 'access-section-body';
	body.append(...controls);
	section.append(intro, body);
}

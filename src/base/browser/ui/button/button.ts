export function button(label: string, className: string, action?: () => void): HTMLButtonElement {
	const element = document.createElement('button');
	element.type = 'button';
	element.className = className;
	element.textContent = label;
	if (action) {
		element.addEventListener('click', action);
	}
	return element;
}

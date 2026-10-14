import './media/settingsControls.css';
import { renderIcon } from '../../../../base/browser/ui/icon/icon.js';
import { Icons } from '../../../../base/common/icons.js';

export function createSetting(label: string, description: string, control: HTMLElement): HTMLElement {
	const row = document.createElement('div');
	row.className = 'setting';
	const caption = document.createElement('label');
	const title = document.createElement('span');
	title.className = 'setting-label';
	title.textContent = label;
	caption.append(title, control);
	const help = document.createElement('p');
	help.className = 'setting-description';
	help.textContent = description;
	row.append(caption, help);
	return row;
}

export function createSelect(options: readonly (readonly [string, string])[]): HTMLSelectElement {
	const select = document.createElement('select');
	for (const [value, label] of options) {
		const option = document.createElement('option');
		option.value = value;
		option.textContent = label;
		select.append(option);
	}
	return select;
}

/** Keep the native popup and keyboard behavior while controlling the arrow gutter. */
export function wrapSelect(select: HTMLSelectElement): HTMLElement {
	const wrapper = document.createElement('span');
	wrapper.className = 'settings-select';
	wrapper.append(select, renderIcon(Icons.chevronDown));
	return wrapper;
}

import * as nls from '../../../../../nls.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import { Emitter } from '../../../../../base/common/event.js';
import { addDisposableListener } from '../../../../../base/browser/domEvent.js';
import type { ConfigurationValues } from '../../../../../platform/configuration/common/configuration.js';

export type ThemeMode = ConfigurationValues['appearance.theme'];

let nextPickerId = 0;

/** Native radio semantics with decorative previews of VniLink's own workbench. */
export class ThemeModePicker extends Disposable {
	public readonly element = document.createElement('fieldset');
	private readonly selected = this._register(new Emitter<ThemeMode>());
	public readonly onDidSelect = this.selected.event;
	private readonly inputs = new Map<ThemeMode, HTMLInputElement>();

	constructor() {
		super();
		this.element.className = 'theme-mode-picker';
		const legend = document.createElement('legend');
		legend.className = 'appearance-visually-hidden';
		legend.textContent = nls.localize('appearance.theme', 'Color theme');
		this.element.append(legend);
		const name = `theme-mode-${String(++nextPickerId)}`;
		const choices: readonly (readonly [ThemeMode, string, string])[] = [
			['system', nls.localize('appearance.system', 'System'), nls.localize('appearance.systemHelp', 'Follow your device')],
			['light', nls.localize('appearance.light', 'Light'), nls.localize('appearance.lightHelp', 'A lighter workspace')],
			['dark', nls.localize('appearance.dark', 'Dark'), nls.localize('appearance.darkHelp', 'A darker workspace')],
		];
		for (const [mode, title, description] of choices) {
			const label = document.createElement('label');
			label.className = 'theme-choice';
			const input = document.createElement('input');
			input.type = 'radio';
			input.name = name;
			input.value = mode;
			input.checked = mode === 'system';
			const preview = document.createElement('span');
			preview.className = `theme-preview theme-preview-${mode}`;
			preview.setAttribute('aria-hidden', 'true');
			preview.append(createWorkbenchPreview(mode === 'light' ? 'light' : 'dark'));
			if (mode === 'system') {
				preview.append(createWorkbenchPreview('light'));
			}
			const caption = document.createElement('span');
			caption.className = 'theme-choice-caption';
			const text = document.createElement('span');
			text.textContent = title;
			caption.append(input, text);
			const help = document.createElement('span');
			help.className = 'theme-choice-help';
			help.textContent = description;
			label.append(preview, caption, help);
			this.element.append(label);
			this.inputs.set(mode, input);
			this._register(addDisposableListener(input, 'change', () => {
				if (input.checked && !input.disabled) {
					this.selected.fire(mode);
				}
			}));
		}
	}

	public get value(): ThemeMode {
		for (const [mode, input] of this.inputs) {
			if (input.checked) {
				return mode;
			}
		}
		return 'system';
	}

	public setValue(mode: ThemeMode): void {
		for (const [key, input] of this.inputs) {
			input.checked = key === mode;
		}
	}

	public setEnabled(enabled: boolean): void {
		for (const input of this.inputs.values()) {
			input.disabled = !enabled;
		}
	}

	public override dispose(): void {
		super.dispose();
		this.inputs.clear();
		this.element.remove();
	}
}

/** Abstract surfaces and marks only; previews do not invent product content. */
function createWorkbenchPreview(mode: 'light' | 'dark'): HTMLElement {
	const surface = document.createElement('span');
	surface.className = `preview-workbench preview-workbench-${mode}`;
	const titlebar = document.createElement('span');
	titlebar.className = 'preview-titlebar';
	const controls = document.createElement('span');
	controls.className = 'preview-controls';
	for (let index = 0; index < 3; index++) {
		controls.append(document.createElement('i'));
	}
	const tab = document.createElement('span');
	tab.className = 'preview-tab';
	tab.append(document.createElement('i'));
	const actions = document.createElement('span');
	actions.className = 'preview-actions';
	actions.append(document.createElement('i'), document.createElement('i'));
	titlebar.append(controls, tab, actions);
	const body = document.createElement('span');
	body.className = 'preview-body';
	const sidebar = document.createElement('span');
	sidebar.className = 'preview-sidebar';
	for (let index = 0; index < 4; index++) {
		const item = document.createElement('span');
		item.className = 'preview-nav-item';
		item.append(document.createElement('i'), document.createElement('span'));
		sidebar.append(item);
	}
	const content = document.createElement('span');
	content.className = 'preview-content';
	const heading = document.createElement('span');
	heading.className = 'preview-heading';
	content.append(heading);
	for (let index = 0; index < 2; index++) {
		const panel = document.createElement('span');
		panel.className = 'preview-panel';
		panel.append(document.createElement('i'), document.createElement('i'), document.createElement('i'));
		content.append(panel);
	}
	body.append(sidebar, content);
	surface.append(titlebar, body);
	return surface;
}

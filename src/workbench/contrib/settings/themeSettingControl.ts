import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingControl } from './settingRenderers.js';
import './media/appearance.css';

type ThemeMode = 'system' | 'light' | 'dark';

let nextPickerId = 0;

/** `appearance.theme` as native radios with decorative previews of VniLink's own workbench. */
export class ThemeSettingControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly inputs = new Map<ThemeMode, HTMLInputElement>();

	public render(container: HTMLElement): void {
		const picker = append(container, $('fieldset.theme-mode-picker'));
		append(picker, $('legend.appearance-visually-hidden')).textContent = nls.localize('appearance.theme', 'Theme');
		const name = `theme-mode-${String(++nextPickerId)}`;
		const choices: readonly (readonly [ThemeMode, string, string])[] = [
			// Light and Dark say what they are; only System needs a word.
			['system', nls.localize('appearance.system', 'System'), nls.localize('appearance.systemHelp', 'Follows your computer')],
			['light', nls.localize('appearance.light', 'Light'), ''],
			['dark', nls.localize('appearance.dark', 'Dark'), ''],
		];
		for (const [mode, title, description] of choices) {
			const choice = append(picker, $('label.theme-choice'));
			const preview = append(choice, $(`span.theme-preview.theme-preview-${mode}`, { 'aria-hidden': 'true' }));
			preview.append(createWorkbenchPreview(mode === 'light' ? 'light' : 'dark'));
			if (mode === 'system') {
				preview.append(createWorkbenchPreview('light'));
			}
			const caption = append(choice, $('span.theme-choice-caption'));
			const input = append(caption, $<HTMLInputElement>('input', { type: 'radio', name, value: mode }));
			append(caption, $('span')).textContent = title;
			if (description) {
				append(choice, $('span.theme-choice-help')).textContent = description;
			}
			this.inputs.set(mode, input);
			this._register(addDisposableListener(input, EventType.CHANGE, () => {
				if (input.checked && !input.disabled) {
					this.changed.fire(mode);
				}
			}));
		}
	}

	public setValue(value: unknown): void {
		for (const [mode, input] of this.inputs) {
			input.checked = mode === value;
		}
	}

	public setEnabled(enabled: boolean): void {
		for (const input of this.inputs.values()) {
			input.disabled = !enabled;
		}
	}
}

/** Abstract surfaces and marks only; previews do not invent product content. */
function createWorkbenchPreview(mode: 'light' | 'dark'): HTMLElement {
	const marks = (count: number): HTMLElement[] => Array.from({ length: count }, () => $('i'));
	const surface = $(`span.preview-workbench.preview-workbench-${mode}`);
	const titlebar = append(surface, $('span.preview-titlebar'));
	append(titlebar, $('span.preview-controls', undefined, ...marks(3)));
	append(titlebar, $('span.preview-tab', undefined, ...marks(1)));
	append(titlebar, $('span.preview-actions', undefined, ...marks(2)));
	const body = append(surface, $('span.preview-body'));
	const sidebar = append(body, $('span.preview-sidebar'));
	for (let index = 0; index < 4; index++) {
		append(sidebar, $('span.preview-nav-item', undefined, $('i'), $('span')));
	}
	const content = append(body, $('span.preview-content'));
	append(content, $('span.preview-heading'));
	for (let index = 0; index < 2; index++) {
		append(content, $('span.preview-panel', undefined, ...marks(3)));
	}
	return surface;
}

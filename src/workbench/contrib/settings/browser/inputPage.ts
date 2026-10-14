import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import { createPageHeading, type WorkbenchPage } from '../../../browser/parts/views/page.js';
import type { TabAction } from '../../../services/tabs/common/tabService.js';
import { IConfigurationService, type InputDefaults } from '../../../../platform/configuration/common/configuration.js';
import { AccessToggle } from './accessToggle.js';
import { createSelect, wrapSelect } from './settingsControls.js';
import './media/access.css';
import './media/input.css';

export class InputPage extends Disposable implements WorkbenchPage {
	public readonly element = document.createElement('main');
	public readonly actions: readonly TabAction[] = [];
	private readonly fields = document.createElement('fieldset');
	private readonly mode = createSelect([['Character', 'Characters'], ['Map', 'Physical keys'], ['Translate', 'Translate to host layout']]);
	private readonly sensitivity = document.createElement('input');
	private readonly value = document.createElement('output');
	private readonly apply = document.createElement('button');
	private readonly status = document.createElement('p');
	private readonly swap = this._register(new AccessToggle(nls.localize('input.swap', 'Swap Control and Command'), nls.localize('input.swapHelp', 'Use familiar shortcuts when moving between Mac and Windows or Linux.'), () => {
		this.updateSave();
	}));
	private readonly wheel = this._register(new AccessToggle(nls.localize('input.wheel', 'Reverse scrolling'), nls.localize('input.wheelHelp', 'Reverse both horizontal and vertical scrolling on the remote computer.'), () => {
		this.updateSave();
	}));
	private readonly buttons = this._register(new AccessToggle(nls.localize('input.buttons', 'Swap primary and secondary buttons'), nls.localize('input.buttonsHelp', 'Use the right mouse button for primary clicks.'), () => {
		this.updateSave();
	}));
	private saved = this.configuration.snapshot.values['input.defaults'];
	private busy = false;

	constructor(@IConfigurationService private readonly configuration: IConfigurationService) {
		super();
		this.element.className = 'input-page';
		this.element.dataset.settingsPage = 'input';
		this.element.append(createPageHeading(nls.localize('home.input', 'Keyboard & Mouse'), nls.localize('input.description', 'Default keyboard mapping and pointer behavior for all remote connections.')));
		const content = document.createElement('div');
		content.className = 'input-content';
		const form = document.createElement('form');
		form.noValidate = true;
		this.fields.className = 'input-fields';
		const keyboard = document.createElement('section');
		const keyboardTitle = document.createElement('h2');
		keyboardTitle.textContent = nls.localize('input.keyboard', 'Keyboard');
		const modeRow = document.createElement('label');
		modeRow.className = 'settings-control-row';
		const modeText = document.createElement('span');
		modeText.textContent = nls.localize('input.mapping', 'Mapping');
		modeRow.append(modeText, wrapSelect(this.mode));
		const help = document.createElement('p');
		help.className = 'settings-help input-mode-help';
		help.textContent = nls.localize('input.mappingHelp', 'Characters sends the text you type. Physical keys uses the remote keyboard layout. Translate sends matching key combinations and requires a Linux X11 host.');
		keyboard.append(keyboardTitle, modeRow, help, this.swap.element);
		const mouse = document.createElement('section');
		const mouseTitle = document.createElement('h2');
		mouseTitle.textContent = nls.localize('input.mouse', 'Mouse');
		const sensitivityRow = document.createElement('label');
		sensitivityRow.className = 'settings-control-row';
		const text = document.createElement('span');
		text.textContent = nls.localize('input.sensitivity', 'Relative pointer sensitivity');
		const control = document.createElement('span');
		control.className = 'input-sensitivity';
		this.sensitivity.type = 'range';
		this.sensitivity.min = '1';
		this.sensitivity.max = '400';
		this.sensitivity.value = '100';
		this.value.textContent = '100%';
		control.append(this.sensitivity, this.value);
		sensitivityRow.append(text, control);
		const pointerHelp = document.createElement('p');
		pointerHelp.className = 'settings-help';
		pointerHelp.textContent = nls.localize('input.relativeHelp', '100% keeps mouse movement unchanged. Absolute pointer positioning is unaffected.');
		mouse.append(mouseTitle, sensitivityRow, pointerHelp, this.wheel.element, this.buttons.element);
		this.fields.append(keyboard, mouse);
		this.fields.disabled = false;
		const footer = document.createElement('div');
		footer.className = 'settings-apply-row';
		this.apply.type = 'submit';
		this.apply.className = 'settings-button primary';
		this.apply.textContent = nls.localize('input.apply', 'Save defaults');
		const note = document.createElement('p');
		note.className = 'settings-help';
		note.textContent = nls.localize('input.sessionHelp', 'Applies to all connected computers and future connections. Release held keys and mouse buttons before saving.');
		footer.append(this.apply, note);
		this.status.className = 'settings-status';
		this.status.setAttribute('role', 'status');
		form.append(this.fields, footer, this.status);
		content.append(form);
		this.element.append(content);
		this._register(addDisposableListener(form, 'input', () => {
			this.value.textContent = `${this.sensitivity.value}%`;
			this.updateSave();
		}));
		this._register(addDisposableListener(form, 'change', () => {
			this.updateSave();
		}));
		this._register(addDisposableListener(form, 'submit', event => {
			event.preventDefault();
			void this.save();
		}));
		this.render(this.saved);
		this.setBusy(false);
		this._register(configuration.onDidChangeConfiguration(() => {
			const clean = !this.dirty();
			this.saved = configuration.snapshot.values['input.defaults'];
			if (!this.busy && clean) {
				this.render(this.saved);
			}
			this.updateSave();
		}));
	}

	private disposed(): boolean {
		return this._store.isDisposed;
	}

	private draft(): InputDefaults {
		return { mode: this.mode.value as InputDefaults['mode'], swap: this.swap.input.checked, pointer: { sensitivity_percent: Number(this.sensitivity.value), reverse_wheel: this.wheel.input.checked, swap_buttons: this.buttons.input.checked } };
	}

	private dirty(): boolean {
		const draft = this.draft();
		const saved = this.saved;
		return draft.mode !== saved.mode || draft.swap !== saved.swap || draft.pointer.sensitivity_percent !== saved.pointer.sensitivity_percent || draft.pointer.reverse_wheel !== saved.pointer.reverse_wheel || draft.pointer.swap_buttons !== saved.pointer.swap_buttons;
	}

	private updateSave(): void {
		this.apply.disabled = this.busy || !this.dirty() || !!this.configuration.snapshot.error;
		this.fields.disabled = this.busy || !!this.configuration.snapshot.error;
		for (const toggle of [this.swap, this.wheel, this.buttons]) {
			toggle.setEnabled(!this.fields.disabled);
		}
		if (this.configuration.snapshot.error) {
			this.status.textContent = this.configuration.snapshot.error;
		}
	}

	private setBusy(busy: boolean): void {
		this.busy = busy;
		this.updateSave();
	}

	private render(settings: InputDefaults): void {
		this.saved = settings;
		this.mode.value = settings.mode;
		this.swap.setValue(settings.swap);
		this.wheel.setValue(settings.pointer.reverse_wheel);
		this.buttons.setValue(settings.pointer.swap_buttons);
		this.sensitivity.value = String(settings.pointer.sensitivity_percent);
		this.value.textContent = `${this.sensitivity.value}%`;
	}

	private async save(): Promise<void> {
		if (this.busy || this.apply.disabled || this.disposed()) {
			return;
		}
		const draft = this.draft();
		this.setBusy(true);
		this.status.textContent = '';
		try {
			await this.configuration.updateValue({ key: 'input.defaults', value: draft });
			if (!this.disposed()) {
				this.render(this.configuration.snapshot.values['input.defaults']);
			}
		} catch {
			if (!this.disposed()) {
				this.status.textContent = nls.localize('input.saveFailed', 'Could not confirm the change. If defaults were saved, restart VniLink to apply them. Otherwise release held keys and buttons and try again.');
			}
		} finally {
			if (!this.disposed()) {
				this.setBusy(false);
			}
		}
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}

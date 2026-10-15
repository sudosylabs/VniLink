import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import type { TabAction } from '../../../services/tabs/common/tabService.js';
import { createPageHeading, type WorkbenchPage } from '../../../browser/parts/views/page.js';
import './settingsControls.js';
import './media/settingsForm.css';

/** Owns one default-settings draft, its save transaction and error recovery. */
export abstract class SettingsForm<T> extends Disposable implements WorkbenchPage {
	public readonly element = document.createElement('main');
	public readonly actions: readonly TabAction[] = [];
	protected readonly fields = document.createElement('fieldset');
	protected readonly content = document.createElement('div');
	protected readonly status = document.createElement('p');
	private readonly save = document.createElement('button');
	private baseline: T | undefined;
	private busy = false;
	private reading = false;
	private revision = 0;

	constructor(id: string, title: string, description: string, note: string) {
		super();
		this.element.className = `${id}-page`;
		this.element.dataset.settingsPage = id;
		this.element.append(createPageHeading(title, description));
		this.content.className = 'settings-form-content';
		const form = document.createElement('form');
		form.noValidate = true;
		this.fields.className = 'settings-form-fields';
		this.fields.disabled = true;
		const footer = document.createElement('div');
		footer.className = 'settings-apply-row';
		this.save.type = 'submit';
		this.save.className = 'settings-button primary';
		this.save.textContent = nls.localize('settings.saveDefaults', 'Save defaults');
		this.save.disabled = true;
		const help = document.createElement('p');
		help.className = 'settings-help';
		help.textContent = note;
		footer.append(this.save, help);
		this.status.className = 'settings-status';
		this.status.setAttribute('role', 'status');
		this.status.textContent = nls.localize('settings.loading', 'Loading settings…');
		this.element.setAttribute('aria-busy', 'true');
		form.append(this.fields, footer, this.status);
		this.content.append(form);
		this.element.append(this.content);
		this._register(addDisposableListener(form, 'input', () => {
			this.status.textContent = '';
			this.update();
		}));
		this._register(addDisposableListener(form, 'change', () => {
			this.status.textContent = '';
			this.update();
		}));
		this._register(addDisposableListener(form, 'submit', event => {
			event.preventDefault();
			void this.apply();
		}));
	}

	protected disposed(): boolean {
		return this._store.isDisposed;
	}

	protected abstract read(): Promise<T>;

	protected abstract write(value: T): Promise<T>;

	protected abstract render(value: T): void;

	protected abstract draft(): T | undefined;

	protected section(title: string, description: string): HTMLElement {
		const section = document.createElement('section');
		const heading = document.createElement('h2');
		heading.textContent = title;
		const help = document.createElement('p');
		help.className = 'settings-help';
		help.textContent = description;
		section.append(heading, help);
		this.fields.append(section);
		return section;
	}

	protected number(section: HTMLElement, key: string, title: string, unit: string, min: number, max: number): HTMLInputElement {
		const row = document.createElement('label');
		row.className = 'settings-control-row';
		const caption = document.createElement('span');
		caption.textContent = title;
		const control = document.createElement('span');
		control.className = 'settings-number';
		const input = document.createElement('input');
		input.type = 'number';
		input.min = String(min);
		input.max = String(max);
		input.step = '1';
		input.dataset.setting = key;
		const suffix = document.createElement('span');
		suffix.textContent = unit;
		control.append(input, suffix);
		row.append(caption, control);
		section.append(row);
		return input;
	}

	protected update(): void {
		this.fields.disabled = this.busy || this.baseline === undefined;
		const draft = this.draft();
		this.save.disabled = this.fields.disabled || draft === undefined || JSON.stringify(draft) === JSON.stringify(this.baseline);
	}

	public async refresh(): Promise<void> {
		if (this.busy || this.reading || this.disposed()) {
			return;
		}
		const initial = this.baseline === undefined;
		const draftBeforeRead = JSON.stringify(this.draft());
		const clean = this.baseline === undefined || draftBeforeRead === JSON.stringify(this.baseline);
		const revision = this.revision;
		this.reading = true;
		try {
			const value = await this.read();
			if (this.disposed() || revision !== this.revision) {
				return;
			}
			this.baseline = value;
			if (clean && (initial || JSON.stringify(value) !== draftBeforeRead) && JSON.stringify(this.draft()) === draftBeforeRead) {
				this.render(value);
			}
			this.status.textContent = '';
		} catch (error) {
			if (!this.disposed() && revision === this.revision) {
				this.baseline = undefined;
				this.status.textContent = error instanceof Error ? error.message : nls.localize('settings.unavailable', 'Settings could not be loaded.');
			}
		} finally {
			this.reading = false;
			if (!this.disposed() && revision === this.revision) {
				this.element.setAttribute('aria-busy', 'false');
				this.update();
			}
		}
	}

	private async apply(): Promise<void> {
		const value = this.draft();
		if (this.busy || this.disposed() || value === undefined || this.save.disabled) {
			return;
		}
		this.revision++;
		this.busy = true;
		this.update();
		this.element.setAttribute('aria-busy', 'true');
		try {
			const saved = await this.write(value);
			if (!this.disposed()) {
				this.baseline = saved;
				this.render(saved);
				this.status.textContent = '';
			}
		} catch (error) {
			if (!this.disposed()) {
				this.status.textContent = error instanceof Error ? error.message : nls.localize('settings.failed', 'Could not save these settings.');
			}
		} finally {
			this.busy = false;
			if (!this.disposed()) {
				this.element.setAttribute('aria-busy', 'false');
				this.update();
			}
		}
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}

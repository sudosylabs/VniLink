import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import { IConfigurationService, type ConfigurationUpdate } from '../../../../platform/configuration/common/configuration.js';
import type { TabAction } from '../../../services/tabs/common/tabService.js';
import { createPageHeading, type WorkbenchPage } from '../../../browser/parts/views/page.js';
import { ThemeModePicker } from './themeModePicker.js';
import './media/appearance.css';

export class AppearancePage extends Disposable implements WorkbenchPage {
	public readonly element = document.createElement('main');
	public readonly actions: readonly TabAction[] = [];
	private readonly theme = this._register(new ThemeModePicker());
	private readonly material = document.createElement('input');
	private readonly status = document.createElement('p');
	private readonly retry = document.createElement('button');
	private failedChange: ConfigurationUpdate | undefined;
	private saving = false;

	constructor(@IConfigurationService private readonly configuration: IConfigurationService) {
		super();
		this.element.className = 'appearance-page';
		this.element.append(createPageHeading(nls.localize('home.appearance', 'Appearance'), nls.localize('appearance.description', 'Make VniLink feel at home on your device.')));
		const form = document.createElement('div');
		form.className = 'appearance-form';
		const themes = document.createElement('section');
		themes.className = 'appearance-section';
		const heading = document.createElement('h2');
		heading.textContent = nls.localize('appearance.theme', 'Color theme');
		const description = document.createElement('p');
		description.className = 'appearance-description';
		description.textContent = nls.localize('appearance.themeDescription', 'Choose a look, or let it change with your system.');
		themes.append(heading, description, this.theme.element);
		const background = document.createElement('section');
		background.className = 'appearance-section appearance-background';
		const label = document.createElement('label');
		label.className = 'appearance-background-row';
		const details = document.createElement('span');
		const title = document.createElement('span');
		title.className = 'appearance-setting-title';
		title.textContent = nls.localize('appearance.material', 'Translucent background');
		const help = document.createElement('span');
		help.className = 'appearance-description';
		help.textContent = nls.localize('appearance.backgroundHelp', 'Blend the sidebar and titlebar with your desktop.');
		details.append(title, help);
		this.material.type = 'checkbox';
		this.material.className = 'appearance-switch';
		this.material.setAttribute('role', 'switch');
		label.append(details, this.material);
		const fallback = document.createElement('p');
		fallback.className = 'appearance-footnote';
		fallback.textContent = nls.localize('appearance.fallback', 'Uses your system’s background effect where available. Reduced transparency and other accessibility preferences take priority.');
		background.append(label, fallback);
		this.retry.type = 'button';
		this.retry.className = 'settings-button';
		this.retry.textContent = nls.localize('settings.retrySave', 'Retry save');
		this.retry.hidden = true;
		this.status.className = 'appearance-status';
		this.status.setAttribute('role', 'status');
		this.status.setAttribute('aria-atomic', 'true');
		this.status.hidden = true;
		form.append(themes, background, this.status, this.retry);
		this.element.append(form);
		this._register(addDisposableListener(this.retry, 'click', () => {
			if (this.failedChange) {
				void this.save(this.failedChange);
			}
		}));
		this._register(this.theme.onDidSelect(value => {
			void this.save({ key: 'appearance.theme', value });
		}));
		this._register(addDisposableListener(this.material, 'change', () => {
			void this.save({ key: 'appearance.nativeBackground', value: this.material.checked });
		}));
		this._register(configuration.onDidChangeConfiguration(() => {
			if (!this.saving && !this.failedChange) {
				this.render();
			}
		}));
		this.render();
	}

	private render(): void {
		this.theme.setValue(this.configuration.snapshot.values['appearance.theme']);
		this.material.checked = this.configuration.snapshot.values['appearance.nativeBackground'];
		this.setControlsEnabled(!this.saving && this.configuration.snapshot.error === null);
		if (this.configuration.snapshot.error) {
			this.status.textContent = this.configuration.snapshot.error;
			this.status.hidden = false;
		}
	}

	private async save(change: ConfigurationUpdate): Promise<void> {
		if (this.saving || this.isDisposed()) {
			return;
		}
		this.saving = true;
		this.element.setAttribute('aria-busy', 'true');
		this.failedChange = undefined;
		this.retry.hidden = true;
		this.setControlsEnabled(false);
		this.status.textContent = '';
		this.status.hidden = true;
		try {
			await this.configuration.updateValue(change);
			if (!this.isDisposed()) {
				this.status.textContent = '';
				this.status.hidden = true;
			}
		} catch {
			if (!this.isDisposed()) {
				this.failedChange = change;
				this.retry.hidden = false;
				this.status.hidden = false;
				this.status.textContent = nls.localize('settings.saveFailed', 'Could not save. The previous preference is still in effect.');
			}
		} finally {
			this.saving = false;
			this.element.setAttribute('aria-busy', 'false');
			if (!this.isDisposed()) {
				if (this.failedChange) {
					this.setControlsEnabled(this.configuration.snapshot.error === null);
				} else {
					this.render();
				}
			}
		}
	}

	private setControlsEnabled(enabled: boolean): void {
		this.theme.setEnabled(enabled);
		this.material.disabled = !enabled;
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}

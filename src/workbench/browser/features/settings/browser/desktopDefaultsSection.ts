import { equalQualityLimits } from '../../../../../platform/engine/common/engine.js';
import * as nls from '../../../../../nls.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../../base/browser/domEvent.js';
import { IConfigurationService } from '../../../../../platform/configuration/common/configuration.js';
import { DesktopQualityEditor } from './desktopQualityEditor.js';

export class DesktopDefaultsSection extends Disposable {
	public readonly element = document.createElement('section');
	private readonly editor = this._register(new DesktopQualityEditor(nls.localize('desktop.defaultQuality', 'Default stream quality')));
	private readonly save = document.createElement('button');
	private readonly status = document.createElement('p');
	private busy = false;
	private baseline = this.configuration.snapshot.values['desktop.quality'];
	constructor(@IConfigurationService private readonly configuration: IConfigurationService) {
		super(); this.element.className = 'desktop-defaults-section';
		const help = document.createElement('p'); help.className = 'settings-help'; help.textContent = nls.localize('desktop.defaultsHelp', 'Default limits for new outgoing desktop sessions. The host may negotiate lower values. Existing sessions keep their current settings.');
		const form = document.createElement('form'); form.noValidate = true; this.save.type = 'submit'; this.save.className = 'settings-button primary'; this.save.textContent = nls.localize('desktop.saveDefaults', 'Save defaults');
		const footer = document.createElement('div'); footer.className = 'settings-apply-row'; footer.append(this.save);
		this.status.className = 'settings-status'; this.status.setAttribute('role', 'status'); form.append(this.editor.element, footer, this.status); this.editor.element.insertBefore(help, this.editor.element.children[1]); this.element.append(form);
		this.editor.setValue(configuration.snapshot.values['desktop.quality']); this.update();
		this._register(this.editor.onDidChange(() => {
			this.status.textContent = ''; this.update();
		}));
		this._register(configuration.onDidChangeConfiguration(() => {
			const clean = !this.dirty();
			this.baseline = configuration.snapshot.values['desktop.quality'];
			if (!this.busy && clean) {
				this.editor.setValue(this.baseline);
			} this.update();
		}));
		this._register(addDisposableListener(form, 'submit', event => {
			event.preventDefault(); void this.saveDefaults();
		}));
	}
	private dirty(): boolean {
		const draft = this.editor.value(); return !!draft && !equalQualityLimits(draft, this.baseline);
	}
	private update(): void {
		this.editor.setEnabled(!this.busy && !this.configuration.snapshot.error); this.save.disabled = this.busy || !this.dirty() || !!this.configuration.snapshot.error; if (this.configuration.snapshot.error) {
			this.status.textContent = this.configuration.snapshot.error;
		}
	}
	private async saveDefaults(): Promise<void> {
		const quality = this.editor.value(); if (this.busy || !quality || !this.dirty() || this._store.isDisposed) {
			return;
		}
		this.busy = true; this.update(); this.status.textContent = '';
		try {
			await this.configuration.updateValue({ key: 'desktop.quality', value: quality }); if (!this.disposed()) {
				this.baseline = this.configuration.snapshot.values['desktop.quality'];
				this.editor.setValue(this.baseline);
			}
		} catch {
			if (!this.disposed()) {
				this.status.textContent = equalQualityLimits(this.configuration.snapshot.values['desktop.quality'], quality) ? nls.localize('desktop.defaultsRuntimeFailed', 'Defaults were saved, but the engine could not confirm them. Restart VniLink before connecting.') : nls.localize('desktop.defaultsFailed', 'Could not save defaults. Your edit is kept; try again.');
			}
		} finally {
			if (!this.disposed()) {
				this.busy = false; this.update();
			}
		}
	}
	private disposed(): boolean {
		return this._store.isDisposed;
	}
}

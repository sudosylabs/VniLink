/** Saved viewer preference. Rust negotiates it when a new Desktop owner opens. */
import * as nls from '../../../../../nls.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../../base/browser/domEvent.js';
import { IConfigurationService } from '../../../../../platform/configuration/common/configuration.js';
import { isCodecPreference } from '../../../../../platform/engine/common/engine.js';
import { createSelect, wrapSelect } from './settingsControls.js';

export class DesktopCodecSection extends Disposable {
	public readonly element = document.createElement('section');
	private readonly select = createSelect([['auto', nls.localize('desktop.codecAuto', 'Auto (recommended)')], ['h264', 'H.264'], ['h265', 'H.265'], ['vp8', 'VP8'], ['vp9', 'VP9'], ['av1', 'AV1']]);
	private readonly save = document.createElement('button');
	private readonly status = document.createElement('p');
	private baseline = this.configuration.snapshot.values['desktop.codec'];
	private busy = false;

	constructor(@IConfigurationService private readonly configuration: IConfigurationService) {
		super();
		this.element.className = 'desktop-processing-section';
		const title = document.createElement('h2');
		title.textContent = nls.localize('desktop.codecTitle', 'Video codec');
		const label = document.createElement('label');
		label.className = 'settings-control-row';
		const caption = document.createElement('span');
		caption.textContent = nls.localize('desktop.codecChoice', 'Preferred codec');
		label.append(caption, wrapSelect(this.select));
		const help = document.createElement('p');
		help.className = 'settings-help';
		help.textContent = nls.localize('desktop.codecHelp', 'Auto chooses a codec supported by both computers. A specific choice requires support on both sides. Changes apply to new desktop sessions.');
		this.save.type = 'submit';
		this.save.className = 'settings-button primary';
		this.save.textContent = nls.localize('desktop.codecSave', 'Save changes');
		const footer = document.createElement('div');
		footer.className = 'settings-apply-row';
		footer.append(this.save);
		const form = document.createElement('form');
		this.status.className = 'settings-status';
		this.status.setAttribute('role', 'status');
		form.append(label, help, footer, this.status);
		this.element.append(title, form);
		this.select.value = this.baseline;
		this.update();
		this._register(addDisposableListener(this.select, 'change', () => {
			this.status.textContent = '';
			this.update();
		}));
		this._register(configuration.onDidChangeConfiguration(() => {
			const clean = this.select.value === this.baseline;
			this.baseline = configuration.snapshot.values['desktop.codec'];
			if (clean && !this.busy) {
				this.select.value = this.baseline;
			}
			this.update();
		}));
		this._register(addDisposableListener(form, 'submit', event => {
			event.preventDefault();
			void this.apply();
		}));
	}

	private disposed(): boolean {
		return this._store.isDisposed;
	}

	private update(): void {
		this.select.disabled = this.busy || !!this.configuration.snapshot.error;
		this.save.disabled = this.select.disabled || this.select.value === this.baseline;
		if (this.configuration.snapshot.error) {
			this.status.textContent = this.configuration.snapshot.error;
		}
	}

	private async apply(): Promise<void> {
		const codec = this.select.value;
		if (this.busy || this._store.isDisposed || codec === this.baseline || !isCodecPreference(codec)) {
			return;
		}
		this.busy = true;
		this.status.textContent = '';
		this.update();
		try {
			await this.configuration.updateValue({ key: 'desktop.codec', value: codec });
			if (!this.disposed()) {
				this.baseline = this.configuration.snapshot.values['desktop.codec'];
				this.select.value = this.baseline;
			}
		} catch (error) {
			if (!this.disposed()) {
				this.status.textContent = error instanceof Error ? error.message : String(error);
			}
		} finally {
			if (!this.disposed()) {
				this.busy = false;
				this.update();
			}
		}
	}
}

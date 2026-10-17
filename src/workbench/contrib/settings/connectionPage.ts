import { IApplicationService } from '../../services/application/applicationService.js';
import { relayDraftChanged } from '../../services/connection/relayConfiguration.js';
import * as nls from '../../../nls.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../base/browser/domEvent.js';
import type { TabAction } from '../../services/tabs/tabService.js';
import { IConnectionService, ConnectionReadbackError, type ConnectionSettings } from '../../services/connection/connectionService.js';
import { validateRelays } from '../../services/connection/relayValidation.js';
import type { RelayInfo } from '../../../platform/engine/common/engine.js';
import { createPageHeading, type WorkbenchPage } from '../../parts/views/page.js';
import { RelayModePicker } from './relayModePicker.js';
import './media/connection.css';
import { RelayEditor } from './relayEditor.js';

export class ConnectionPage extends Disposable implements WorkbenchPage {
	public readonly element = document.createElement('main');
	public readonly actions: readonly TabAction[] = [];
	private readonly form = document.createElement('form');
	private readonly fields = document.createElement('fieldset');
	private readonly mode = this._register(new RelayModePicker());
	private readonly custom = document.createElement('div');
	private readonly rowsContainer = document.createElement('div');
	private readonly rows: RelayEditor[] = [];
	private readonly add = document.createElement('button');
	private readonly saveButton = document.createElement('button');
	private readonly retry = document.createElement('button');
	private readonly status = document.createElement('p');
	private readonly active = document.createElement('div');
	private readonly publicHelp = document.createElement('p');
	private readonly pending = document.createElement('p');
	private readonly runningState = document.createElement('span');
	private saved: ConnectionSettings | undefined;
	private busy = false;
	private customValidation = false;

	constructor(@IConnectionService private readonly connection: IConnectionService, @IApplicationService private readonly application: IApplicationService) {
		super();
		this.element.dataset.settingsPage = 'connection';
		this.element.className = 'connection-page';
		this.element.append(createPageHeading(nls.localize('home.connection', 'Connection'), nls.localize('home.connectionDescription', 'Relays and how your devices connect.')));
		this.form.className = 'settings-form connection-form';
		this.form.noValidate = true;
		this.fields.className = 'settings-fields';
		this.add.type = 'button';
		this.add.className = 'settings-button';
		this.add.textContent = nls.localize('connection.addRelay', 'Add relay');
		this.saveButton.type = 'submit';
		this.saveButton.className = 'settings-button primary';
		this.saveButton.textContent = nls.localize('connection.saveChanges', 'Save changes');
		this.retry.type = 'button';
		this.retry.className = 'settings-button';
		this.retry.textContent = nls.localize('settings.retry', 'Reload settings');
		this.retry.hidden = true;
		this.status.className = 'settings-status';
		this.status.setAttribute('role', 'status');
		this.active.className = 'connection-active-description';
		const modeSection = document.createElement('section');
		modeSection.className = 'connection-section connection-mode-section';
		const modeHeading = document.createElement('h2');
		modeHeading.textContent = nls.localize('connection.mode', 'Relay mode');
		const modeControls = document.createElement('div');
		modeControls.className = 'connection-mode-controls';
		modeControls.append(modeHeading, this.mode.element);
		this.publicHelp.className = 'connection-description connection-public-help';
		const explanation = document.createTextNode(nls.localize('connection.publicHelp', 'Uses Iroh’s public relays, operated by n0.computer. Relayed traffic is rate-limited; direct connections bypass these limits. '));
		const documentation = document.createElement('a');
		documentation.href = 'https://docs.iroh.computer/relays/rate-limiting';
		documentation.textContent = nls.localize('connection.publicDocs', 'Read Iroh’s relay documentation');
		this._register(addDisposableListener(documentation, 'click', event => {
			event.preventDefault();
			void this.application.openRelayDocumentation().catch(() => {
				this.customValidation = false;
				this.status.textContent = nls.localize('connection.docsFailed', 'Could not open the relay documentation.');
			});
		}));
		this.publicHelp.append(explanation, documentation);
		modeSection.append(modeControls, this.publicHelp);
		this.custom.className = 'connection-section connection-custom';
		const customHeader = document.createElement('div');
		customHeader.className = 'connection-section-header';
		const customTitle = document.createElement('h2');
		customTitle.textContent = nls.localize('connection.servers', 'Relay servers');
		customHeader.append(customTitle, this.add);
		const help = document.createElement('p');
		help.className = 'connection-description';
		help.textContent = nls.localize('connection.customDescription', 'Up to eight servers. Use HTTPS, or HTTP for a loopback address.');
		this.custom.append(customHeader, help, this.rowsContainer);
		const footer = document.createElement('div');
		footer.className = 'connection-save-row';
		const restart = document.createElement('p');
		restart.className = 'connection-description';
		restart.textContent = nls.localize('connection.restartHelp', 'Relay changes take effect after restarting VniLink.');
		footer.append(restart, this.saveButton);
		this.fields.append(modeSection, this.custom, footer);
		const current = document.createElement('section');
		current.className = 'connection-section connection-current';
		const currentLabel = document.createElement('h2');
		currentLabel.className = 'connection-current-label';
		currentLabel.textContent = nls.localize('connection.inUse', 'Running configuration');
		this.pending.className = 'connection-description';
		this.pending.hidden = true;
		this.pending.textContent = nls.localize('connection.pending', 'Your saved configuration will take effect after restarting VniLink. Use the restart action in the titlebar.');
		const currentHeader = document.createElement('div');
		currentHeader.className = 'connection-section-header';
		this.runningState.className = 'connection-runtime-state';
		currentHeader.append(currentLabel, this.runningState);
		current.append(currentHeader, this.active, this.pending);
		this.form.append(this.fields, this.status, this.retry, current);
		this.element.append(this.form);
		this._register(this.mode.onDidChange(() => {
			this.updateMode();
		}));
		this._register(addDisposableListener(this.add, 'click', () => {
			this.addRow();
		}));
		this._register(addDisposableListener(this.retry, 'click', () => {
			void this.load();
		}));
		this._register(addDisposableListener(this.form, 'submit', event => {
			event.preventDefault();
			void this.save();
		}));
		this._register(addDisposableListener(this.form, 'input', () => {
			this.updateSave();
		}));
		this._register(addDisposableListener(this.form, 'change', () => {
			this.updateSave();
		}));
		this.saveButton.disabled = true;
		if (connection.available) {
			void this.load();
		} else {
			this.fields.disabled = true;
			this.custom.hidden = true;
			this.status.textContent = nls.localize('connection.unavailable', 'Connection settings are unavailable because the connection engine could not start. Restart VniLink to try again.');
		}
	}

	private addRow(relay?: RelayInfo): void {
		if (this.rows.length >= 8) {
			return;
		}
		const row = new RelayEditor(relay, () => {
			this.rows.splice(this.rows.indexOf(row), 1);
			row.dispose();
			this.updateMode();
		});
		this.rows.push(row);
		this.rowsContainer.append(row.element);
		this.updateMode();
	}

	private updateMode(): void {
		this.custom.hidden = this.mode.value !== 'custom';
		if (this.customValidation && this.mode.value !== 'custom') {
			this.status.textContent = '';
			this.customValidation = false;
		}
		this.add.disabled = this.rows.length >= 8;
		this.publicHelp.hidden = this.mode.value !== 'public';
		this.updateSave();
		if (this.mode.value === 'custom' && !this.rows.length) {
			this.addRow();
		}
	}

	private render(settings: ConnectionSettings): void {
		this.saved = settings;
		for (const row of this.rows.splice(0)) {
			row.dispose();
		}
		this.mode.setValue(settings.saved.length ? 'custom' : 'public');
		for (const relay of settings.saved) {
			this.addRow(relay);
		}
		this.updateMode();
		this.active.replaceChildren();
		const label = document.createElement('p');
		label.className = 'connection-active-mode';
		label.textContent = settings.active.publicRelay ? nls.localize('connection.activePublic', 'Iroh public relays') : nls.localize('connection.activeCustom', 'Custom relays');
		this.active.append(label);
		if (!settings.active.publicRelay) {
			const list = document.createElement('ul');
			list.className = 'connection-active-urls';
			for (const url of settings.active.urls) {
				const item = document.createElement('li');
				item.textContent = url;
				list.append(item);
			}
			this.active.append(list);
		}
		this.pending.hidden = !settings.restartRequired;
		this.runningState.textContent = settings.restartRequired ? nls.localize('connection.restartRequired', 'Restart required') : nls.localize('connection.running', 'Running');
		this.runningState.dataset.pending = String(settings.restartRequired);
	}

	private async load(): Promise<void> {
		if (this.busy || this.isDisposed()) {
			return;
		}
		this.busy = true;
		this.fields.disabled = this.retry.disabled = true;
		this.customValidation = false;
		this.status.textContent = nls.localize('settings.loading', 'Loading settings…');
		try {
			const settings = await this.connection.load();
			if (this.isDisposed()) {
				return;
			}
			this.render(settings);
			this.status.textContent = '';
			this.retry.hidden = true;
		} catch {
			if (!this.isDisposed()) {
				this.status.textContent = nls.localize('connection.loadFailed', 'Could not load connection settings. Your saved configuration has not been changed.');
				this.retry.hidden = false;
			}
		} finally {
			this.busy = false;
			if (!this.isDisposed()) {
				this.fields.disabled = !this.saved;
				this.updateSave();
				this.retry.disabled = false;
			}
		}
	}

	private async save(): Promise<void> {
		if (this.busy || !this.saved || !this.dirty() || this.isDisposed()) {
			return;
		}
		const relays = this.rows.map(row => row.value);
		const issues = this.mode.value === 'custom' ? validateRelays(relays, new Set(this.saved.saved.filter(relay => relay.authenticated).map(relay => relay.url))) : [];
		for (const [index, row] of this.rows.entries()) {
			row.showIssues(issues.filter(issue => issue.index === index));
		}
		if (issues.length) {
			this.customValidation = true;
			this.status.textContent = issues[0].message;
			return;
		}
		this.customValidation = false;
		this.busy = true;
		this.fields.disabled = true;
		this.status.textContent = nls.localize('settings.saving', 'Saving…');
		try {
			const settings = await this.connection.save(this.mode.value === 'public' ? { mode: 'public' } : { mode: 'custom', relays });
			if (this.isDisposed()) {
				return;
			}
			this.render(settings);
			this.retry.hidden = true;
			this.status.textContent = '';
		} catch (error) {
			if (!this.isDisposed()) {
				if (error instanceof ConnectionReadbackError) {
					for (const row of this.rows) {
						row.clearToken();
					}
					this.saved = undefined;
					this.retry.hidden = false;
					this.status.textContent = nls.localize('connection.readbackFailed', 'Saved, but the updated settings could not be loaded. Reload settings before editing again.');
				} else {
					this.status.textContent = nls.localize('connection.saveFailed', 'Could not save. Your previous relay configuration is unchanged; check the relay details and try again.');
				}
			}
		} finally {
			this.busy = false;
			if (!this.isDisposed()) {
				this.fields.disabled = !this.saved;
				this.updateSave();
			}
		}
	}

	private dirty(): boolean {
		return !!this.saved && relayDraftChanged(this.saved.saved, this.mode.value === 'public' ? { mode: 'public' } : { mode: 'custom', relays: this.rows.map(row => row.value) });
	}

	private updateSave(): void {
		this.saveButton.disabled = this.busy || !this.dirty();
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}

	public override dispose(): void {
		super.dispose();
		for (const row of this.rows.splice(0)) {
			row.dispose();
		}
		this.element.remove();
	}
}

import * as nls from '../../../nls.js';
import { $, append } from '../../../base/browser/dom.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { IApplicationService } from '../../services/application/applicationService.js';
import { IConnectionService, type RunningRelays } from '../../services/connection/connectionService.js';
import { IWorkbenchConfigurationService } from '../../services/configuration/configurationService.js';
import type { ISettingsWidget } from './settingsLayout.js';

/** The relays the running engine uses, and whether saved relays wait for a restart. */
export class RunningRelaysWidget extends Disposable implements ISettingsWidget {
	private readonly active = $('.connection-active-description');
	private readonly state = $('span.connection-runtime-state');
	private readonly pending = $('p.connection-description');
	private revision = 0;

	constructor(
		@IConnectionService private readonly connectionService: IConnectionService,
		@IApplicationService private readonly applicationService: IApplicationService,
		@IWorkbenchConfigurationService private readonly configurationService: IWorkbenchConfigurationService,
	) {
		super();
	}

	public render(container: HTMLElement): void {
		const section = append(container, $('section.connection-section.connection-current'));
		const header = append(section, $('.connection-section-header'));
		append(header, $('h2.connection-current-label')).textContent = nls.localize('connection.inUse', 'Running configuration');
		header.append(this.state);
		this.pending.textContent = nls.localize('connection.pending', 'Your saved configuration will take effect after restarting VniLink. Use the restart action in the titlebar.');
		section.append(this.active, this.pending);
		this._register(this.applicationService.onDidChangeRestartRequired(() => {
			this.renderRestart();
		}));
		this._register(this.configurationService.onDidChangeConfiguration(event => {
			if (event.affectsConfiguration('connection.relays')) {
				void this.refresh();
			}
		}));
		this.renderRestart();
	}

	public async refresh(): Promise<void> {
		const revision = ++this.revision;
		try {
			const relays = await this.connectionService.getRunningRelays();
			if (!this.isDisposed() && revision === this.revision) {
				this.renderRelays(relays);
			}
		} catch {
			if (!this.isDisposed() && revision === this.revision) {
				this.active.replaceChildren();
			}
		}
	}

	private renderRelays(relays: RunningRelays): void {
		const mode = $('p.connection-active-mode');
		mode.textContent = relays.publicRelay ? nls.localize('connection.activePublic', 'Iroh public relays') : nls.localize('connection.activeCustom', 'Custom relays');
		this.active.replaceChildren(mode);
		if (!relays.publicRelay) {
			const list = append(this.active, $('ul.connection-active-urls'));
			for (const url of relays.urls) {
				append(list, $('li')).textContent = url;
			}
		}
	}

	private renderRestart(): void {
		const required = this.applicationService.restartRequired;
		this.pending.hidden = !required;
		this.state.textContent = required ? nls.localize('connection.restartRequired', 'Restart required') : nls.localize('connection.running', 'Running');
		this.state.dataset.pending = String(required);
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}

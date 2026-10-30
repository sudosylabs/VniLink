// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import type { RelayInfo, RelayInput } from '../../../platform/engine/common/engine.js';
import { relaysChanged, validateRelays } from '../../../platform/engine/common/relayValidation.js';
import { IApplicationService } from '../../services/application/applicationService.js';
import { RelayEditor } from './relayEditor.js';
import { RelayModePicker } from './relayModePicker.js';
import type { ISettingControl } from './settingRenderers.js';
import './media/connection.css';

const maxRelays = 8;

/**
 * `connection.relays`: public relays, or up to eight custom ones. The list is
 * edited as a local draft and committed whole once every relay is valid, so a
 * half-typed address never reaches the engine; saved tokens are never shown.
 */
export class RelaysSettingControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly mode = this._register(new RelayModePicker());
	private readonly custom = $('.connection-section.connection-custom');
	private readonly rowsContainer = $('div');
	private readonly add = $<HTMLButtonElement>('button.settings-button', { type: 'button' });
	private readonly publicHelp = $('p.connection-description.connection-public-help');
	private readonly message = $('p.setting-error');
	private readonly rows: RelayEditor[] = [];
	private saved: readonly RelayInfo[] = [];
	private enabled = false;

	constructor(_key: string, _schema: ISettingSchema, @IApplicationService private readonly applicationService: IApplicationService) {
		super();
	}

	public render(container: HTMLElement): void {
		const fields = append(container, $('.settings-fields'));
		const modeSection = append(fields, $('section.connection-section.connection-mode-section'));
		const modeControls = append(modeSection, $('.connection-mode-controls'));
		append(modeControls, $('h2')).textContent = nls.localize('connection.mode', 'Relay mode');
		modeControls.append(this.mode.element);
		this.publicHelp.append(nls.localize('connection.publicHelp', 'Uses Iroh’s public relays, operated by n0.computer. Relayed traffic is rate-limited; direct connections bypass these limits. '));
		const documentation = append(this.publicHelp, $<HTMLAnchorElement>('a', { href: 'https://docs.iroh.computer/relays/rate-limiting' }));
		documentation.textContent = nls.localize('connection.publicDocs', 'Read Iroh’s relay documentation');
		modeSection.append(this.publicHelp);
		fields.append(this.custom);
		const header = append(this.custom, $('.connection-section-header'));
		append(header, $('h2')).textContent = nls.localize('connection.servers', 'Relay servers');
		this.add.textContent = nls.localize('connection.addRelay', 'Add relay');
		header.append(this.add);
		append(this.custom, $('p.connection-description')).textContent = nls.localize('connection.customDescription', 'Up to eight servers. Use HTTPS, or HTTP for a loopback address.');
		this.custom.append(this.rowsContainer, this.message);
		this.message.hidden = true;
		const footer = append(fields, $('.connection-save-row'));
		append(footer, $('p.connection-description')).textContent = nls.localize('connection.restartHelp', 'Relay changes take effect after restarting VniLink.');
		this._register(addDisposableListener(documentation, EventType.CLICK, event => {
			event.preventDefault();
			this.applicationService.openRelayDocumentation().catch(() => {
				this.showMessage(nls.localize('connection.docsFailed', 'Could not open the relay documentation.'));
			});
		}));
		this._register(this.mode.onDidChange(() => {
			if (this.mode.value === 'custom' && !this.rows.length) {
				this.addRow();
			}
			this.updateMode();
			this.commit();
		}));
		this._register(addDisposableListener(this.add, EventType.CLICK, () => {
			this.addRow();
			this.updateMode();
		}));
		this.updateMode();
	}

	private addRow(relay?: RelayInfo): void {
		if (this.rows.length >= maxRelays) {
			return;
		}
		const row = new RelayEditor(relay);
		this.rows.push(row);
		this.rowsContainer.append(row.element);
		row.setEnabled(this.enabled);
		row.onDidChange(() => {
			this.commit();
		});
		row.onDidRemove(() => {
			this.rows.splice(this.rows.indexOf(row), 1);
			row.dispose();
			this.updateMode();
			this.commit();
		});
	}

	private draft(): RelayInput[] {
		return this.mode.value === 'public' ? [] : this.rows.map(row => row.value);
	}

	/** Proposes the draft once it is complete, valid and different from what is saved. */
	private commit(): void {
		this.showMessage('');
		const draft = this.draft();
		const issues = this.mode.value === 'custom' ? validateRelays(draft, new Set(this.saved.filter(relay => relay.authenticated).map(relay => relay.url))) : [];
		for (const [index, row] of this.rows.entries()) {
			row.showIssues(issues.filter(issue => issue.index === index));
		}
		const general = issues.find(issue => issue.index < 0);
		if (general) {
			this.showMessage(general.message);
		}
		if (!issues.length && relaysChanged(this.saved, draft)) {
			this.changed.fire(draft);
		}
	}

	public setValue(value: unknown): void {
		const saved: readonly RelayInfo[] = Array.isArray(value) ? value : [];
		const clean = !relaysChanged(this.saved, this.draft());
		const committed = !relaysChanged(saved, this.draft().map(relay => ({ ...relay, replacementToken: '' })));
		this.saved = saved;
		if (committed) {
			// The draft is what is saved now: its new tokens are stored.
			for (const [index, row] of this.rows.entries()) {
				row.setSavedToken(saved[index]?.authenticated ?? false);
			}
		} else if (clean) {
			for (const row of this.rows.splice(0)) {
				row.dispose();
			}
			this.mode.setValue(saved.length ? 'custom' : 'public');
			for (const relay of saved) {
				this.addRow(relay);
			}
		}
		this.updateMode();
	}

	public setEnabled(enabled: boolean): void {
		this.enabled = enabled;
		this.mode.element.disabled = !enabled;
		for (const row of this.rows) {
			row.setEnabled(enabled);
		}
		this.updateMode();
	}

	private updateMode(): void {
		this.custom.hidden = this.mode.value !== 'custom';
		this.publicHelp.hidden = this.mode.value !== 'public';
		this.add.disabled = !this.enabled || this.rows.length >= maxRelays;
	}

	private showMessage(message: string): void {
		this.message.textContent = message;
		this.message.hidden = !message;
	}

	public override dispose(): void {
		for (const row of this.rows.splice(0)) {
			row.dispose();
		}
		super.dispose();
	}
}

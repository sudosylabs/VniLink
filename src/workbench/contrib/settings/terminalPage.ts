import * as nls from '../../../nls.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../base/browser/domEvent.js';
import type { TerminalPolicy } from '../../../platform/engine/common/engine.js';
import { ITerminalSettingsService, validTerminalPolicy } from '../../services/terminal/terminalSettingsService.js';
import { createPageHeading, type WorkbenchPage } from '../../parts/views/page.js';
import type { TabAction } from '../../services/tabs/tabService.js';
import { AccessToggle } from './accessToggle.js';
import './settingsControls.js';
import './media/access.css';
import './media/files.css';

type LimitKey = Exclude<keyof TerminalPolicy, 'allow_persistence'>;

export class TerminalPage extends Disposable implements WorkbenchPage {
	public readonly element = document.createElement('main');
	public readonly actions: readonly TabAction[] = [];
	private readonly fields = document.createElement('fieldset');
	private readonly refresh = document.createElement('button');
	private readonly save = document.createElement('button');
	private readonly status = document.createElement('p');
	private readonly numbers = new Map<LimitKey, { input: HTMLInputElement; scale: number }>();
	private readonly persistence = this._register(new AccessToggle(nls.localize('terminal.persistence', 'Keep detached terminals running'), nls.localize('terminal.persistenceHelp', 'Allow a device to leave a terminal running and reconnect to it later.'), () => {
		this.update();
	}));
	private baseline: Readonly<TerminalPolicy> | undefined;
	private busy = false;

	constructor(@ITerminalSettingsService private readonly terminal: ITerminalSettingsService) {
		super();
		this.element.className = 'terminal-page';
		this.element.dataset.settingsPage = 'terminal';
		this.element.append(createPageHeading(nls.localize('home.terminal', 'Terminal'), nls.localize('terminal.description', 'Limits for remote terminals hosted on this computer.')));
		const content = document.createElement('div');
		content.className = 'terminal-content';
		const form = document.createElement('form');
		form.noValidate = true;
		this.fields.className = 'terminal-fields';
		const sessions = document.createElement('section');
		const header = document.createElement('div');
		header.className = 'settings-section-header';
		const title = document.createElement('h2');
		title.textContent = nls.localize('terminal.sessions', 'Session limits');
		this.refresh.type = 'button';
		this.refresh.className = 'settings-text-button';
		this.refresh.textContent = nls.localize('terminal.refresh', 'Refresh');
		header.append(title, this.refresh);
		const help = document.createElement('p');
		help.className = 'settings-help';
		help.textContent = nls.localize('terminal.help', 'A device needs Terminal permission and an available shell backend. Saving these limits does not launch a shell. Close hosted terminals before changing them.');
		sessions.append(help);
		sessions.append(this.number('max_sessions', nls.localize('terminal.total', 'Maximum terminals'), nls.localize('terminal.totalHelp', 'Total across all paired devices.'), 1, 16), this.number('max_sessions_per_peer', nls.localize('terminal.perDevice', 'Maximum per device'), nls.localize('terminal.perDeviceHelp', 'Cannot exceed the total limit.'), 1, 16));
		const reconnect = document.createElement('section');
		const reconnectTitle = document.createElement('h2');
		reconnectTitle.textContent = nls.localize('terminal.reconnect', 'Disconnect & reconnect');
		reconnect.append(reconnectTitle, this.persistence.element, this.number('detached_timeout_secs', nls.localize('terminal.timeout', 'Detached lifetime'), nls.localize('terminal.timeoutHelp', 'Stop a detached terminal after this many seconds. Used when persistence is allowed.'), 1, 86400, 's'));
		const history = document.createElement('section');
		const historyTitle = document.createElement('h2');
		historyTitle.textContent = nls.localize('terminal.history', 'Output history');
		history.append(historyTitle, this.number('history_bytes', nls.localize('terminal.historySize', 'History limit'), nls.localize('terminal.historyHelp', 'Maximum output retained per terminal, from 16 KiB to 4 MiB.'), 16, 4096, 'KiB', 1024), this.number('replay_bytes', nls.localize('terminal.replay', 'Replay on reconnect'), nls.localize('terminal.replayHelp', 'Output replayed when a device reconnects. Cannot exceed the history limit; zero disables replay.'), 0, 4096, 'KiB', 1024));
		this.fields.append(sessions, reconnect, history);
		const footer = document.createElement('div');
		footer.className = 'settings-apply-row';
		this.save.type = 'submit';
		this.save.className = 'settings-button primary';
		this.save.textContent = nls.localize('terminal.save', 'Save changes');
		footer.append(this.save);
		this.status.className = 'settings-status';
		this.status.setAttribute('role', 'status');
		form.append(header, this.fields, footer, this.status);
		content.append(form);
		this.element.append(content);
		this._register(addDisposableListener(this.refresh, 'click', () => {
			void this.load();
		}));
		this._register(addDisposableListener(form, 'input', () => {
			this.update();
		}));
		this._register(addDisposableListener(form, 'submit', event => {
			event.preventDefault();
			void this.savePolicy();
		}));
		this.update();
		if (terminal.available) {
			void this.load();
		} else {
			this.status.textContent = nls.localize('terminal.unavailable', 'The connection engine is unavailable. Restart VniLink to try again.');
			this.refresh.disabled = true;
		}
	}

	private number(key: LimitKey, title: string, help: string, min: number, max: number, unit = '', scale = 1): HTMLElement {
		const row = document.createElement('label');
		row.className = 'settings-control-row';
		const caption = document.createElement('span');
		const name = document.createElement('span');
		name.className = 'access-toggle-title';
		name.textContent = title;
		const description = document.createElement('span');
		description.className = 'settings-help';
		description.textContent = help;
		caption.append(name, description);
		const input = document.createElement('input');
		input.type = 'number';
		input.min = String(min);
		input.max = String(max);
		input.step = scale === 1 ? '1' : 'any';
		input.dataset.limit = key;
		this.numbers.set(key, { input, scale });
		const control = document.createElement('span');
		control.className = 'terminal-number';
		const suffix = document.createElement('span');
		suffix.textContent = unit;
		control.append(input, suffix);
		row.append(caption, control);
		return row;
	}

	private numberValue(key: LimitKey): number {
		const row = this.numbers.get(key);
		return row && row.input.value !== '' ? Number(row.input.value) * row.scale : NaN;
	}

	private draft(): TerminalPolicy {
		return { allow_persistence: this.persistence.input.checked, max_sessions: this.numberValue('max_sessions'), max_sessions_per_peer: this.numberValue('max_sessions_per_peer'), history_bytes: this.numberValue('history_bytes'), replay_bytes: this.numberValue('replay_bytes'), detached_timeout_secs: this.numberValue('detached_timeout_secs') };
	}

	private render(policy: Readonly<TerminalPolicy>): void {
		this.baseline = policy;
		this.persistence.setValue(policy.allow_persistence);
		for (const [key, row] of this.numbers) {
			row.input.value = String(policy[key] / row.scale);
		}
	}

	private update(): void {
		this.fields.disabled = this.busy || !this.baseline;
		this.persistence.setEnabled(!this.fields.disabled);
		this.refresh.disabled = this.busy;
		this.save.disabled = this.fields.disabled || JSON.stringify(this.draft()) === JSON.stringify(this.baseline);
	}

	private async load(): Promise<void> {
		if (this.busy || this.disposed()) {
			return;
		}
		this.busy = true;
		this.baseline = undefined;
		this.update();
		this.status.textContent = '';
		try {
			const policy = await this.terminal.load();
			if (!this.disposed()) {
				this.render(policy);
			}
		} catch {
			if (!this.disposed()) {
				this.status.textContent = nls.localize('terminal.loadFailed', 'Could not load terminal settings. Refresh to try again.');
			}
		} finally {
			if (!this.disposed()) {
				this.busy = false;
				this.update();
			}
		}
	}

	private async savePolicy(): Promise<void> {
		if (this.busy || !this.baseline || this.save.disabled || this.disposed()) {
			return;
		}
		const policy = this.draft();
		if (!validTerminalPolicy(policy)) {
			this.status.textContent = nls.localize('terminal.invalid', 'Check the limits: 1–16 terminals, per-device limit no higher than the total, history from 16 KiB to 4 MiB, replay no larger than history, and lifetime from 1 to 86,400 seconds.');
			return;
		}
		this.busy = true;
		this.update();
		this.status.textContent = '';
		try {
			const saved = await this.terminal.save(policy);
			if (!this.disposed()) {
				this.render(saved);
			}
		} catch {
			if (!this.disposed()) {
				this.baseline = undefined;
				this.status.textContent = nls.localize('terminal.saveFailed', 'Could not confirm the change. Close hosted terminals, then refresh before editing again.');
			}
		} finally {
			if (!this.disposed()) {
				this.busy = false;
				this.update();
			}
		}
	}

	private disposed(): boolean {
		return this._store.isDisposed;
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}

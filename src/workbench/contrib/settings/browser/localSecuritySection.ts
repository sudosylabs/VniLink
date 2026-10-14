import { appendAccessSection } from './accessSection.js';
import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import type { SystemPolicy, SystemSupport } from '../../../../platform/engine/common/engine.js';
import { IAccessService, AccessReadbackError, type LocalSecurity } from '../../../services/access/common/accessService.js';
import { AccessToggle } from './accessToggle.js';

export class LocalSecuritySection extends Disposable {
	public readonly element = document.createElement('section');
	private readonly refreshButton = document.createElement('button');
	private readonly status = document.createElement('p');
	private readonly rows = new Map<keyof SystemPolicy, AccessToggle>();
	private current: LocalSecurity | undefined;
	private busy = false;
	private readonly definitions: readonly (readonly [keyof SystemPolicy, keyof SystemSupport, string, string])[] = [
		['allow_lock', 'lock', nls.localize('security.lock', 'Allow remote lock'), nls.localize('security.lockHelp', 'Let permitted devices lock this computer.')],
		['allow_restart', 'restart', nls.localize('security.restart', 'Allow remote restart'), nls.localize('security.restartHelp', 'Let permitted devices restart this computer.')],
		['allow_shutdown', 'shutdown', nls.localize('security.shutdown', 'Allow remote shutdown'), nls.localize('security.shutdownHelp', 'Let permitted devices shut down this computer.')],
		['allow_privacy', 'privacy', nls.localize('security.privacy', 'Allow privacy mode'), nls.localize('security.privacyHelp', 'Let permitted devices request privacy mode during remote sessions.')],
	];

	constructor(@IAccessService private readonly access: IAccessService) {
		super();
		this.element.className = 'access-section';
		const header = document.createElement('div');
		header.className = 'access-section-header';
		const heading = document.createElement('h2');
		heading.textContent = nls.localize('security.local', 'Remote action rules');
		this.refreshButton.type = 'button';
		this.refreshButton.className = 'access-text-button';
		this.refreshButton.textContent = nls.localize('access.refresh', 'Refresh');
		header.append(heading, this.refreshButton);
		const help = document.createElement('p');
		help.className = 'access-description';
		help.textContent = nls.localize('security.policyHelp', 'Rules for remote requests, separate from operating-system permissions. A device also needs the System actions permission.');
		const controlsBody = document.createElement('div');
		appendAccessSection(this.element, header, help, [controlsBody]);
		for (const [key, , title, description] of this.definitions) {
			const row = this._register(new AccessToggle(title, description, allowed => {
				void this.change(key, allowed);
			}));
			row.input.dataset.policy = key;
			this.rows.set(key, row);
			controlsBody.append(row.element);
		}
		this.status.className = 'access-status';
		this.status.setAttribute('role', 'status');
		controlsBody.append(this.status);
		this._register(addDisposableListener(this.refreshButton, 'click', () => {
			void this.load();
		}));
		if (access.available) {
			void this.load();
		} else {
			this.refreshButton.disabled = true;
			this.status.textContent = nls.localize('security.unavailable', 'Security policy is unavailable while the connection engine is stopped.');
		}
	}

	private disposed(): boolean {
		return this._store.isDisposed;
	}

	private setBusy(busy: boolean): void {
		this.busy = busy;
		this.element.setAttribute('aria-busy', String(busy));
		this.refreshButton.disabled = busy;
		for (const row of this.rows.values()) {
			const capability = this.definitions.find(([key]) => this.rows.get(key) === row)?.[1];
			row.setEnabled(!busy && !!this.current && !!capability && this.current.support[capability]);
		}
	}

	private render(state: LocalSecurity): void {
		this.current = state;
		for (const [key, capability, , description] of this.definitions) {
			const row = this.rows.get(key);
			row?.setValue(state.policy[key]);
			row?.setDescription(state.support[capability] ? description : nls.localize('security.unsupported', '{0} Not supported on this computer.', description));
		}
	}

	private async load(): Promise<void> {
		if (this.busy || this.disposed()) {
			return;
		}
		this.setBusy(true);
		try {
			const state = await this.access.loadPolicy();
			if (!this.disposed()) {
				this.render(state);
				this.status.textContent = '';
			}
		} catch {
			if (!this.disposed()) {
				this.current = undefined;
				this.status.textContent = nls.localize('security.loadFailed', 'Could not load security policy. Refresh to try again.');
			}
		} finally {
			if (!this.disposed()) {
				this.setBusy(false);
			}
		}
	}

	private async change(key: keyof SystemPolicy, allowed: boolean): Promise<void> {
		const capability = this.definitions.find(([policy]) => policy === key)?.[1];
		if (this.busy || !this.current || !capability || !this.current.support[capability] || this.disposed()) {
			return;
		}
		const previous = this.current;
		this.setBusy(true);
		this.status.textContent = '';
		try {
			const state = await this.access.setPolicy(key, allowed);
			if (!this.disposed()) {
				this.render(state);
			}
		} catch (error) {
			if (!this.disposed()) {
				if (error instanceof AccessReadbackError) {
					this.current = undefined;
					this.status.textContent = nls.localize('security.readbackFailed', 'Policy changed, but could not be reloaded. Refresh before editing again.');
				} else {
					this.render(previous);
					this.current = undefined;
					this.status.textContent = nls.localize('security.changeFailed', 'Could not confirm the change. Refresh to check the current policy.');
				}
			}
		} finally {
			if (!this.disposed()) {
				this.setBusy(false);
			}
		}
	}
}

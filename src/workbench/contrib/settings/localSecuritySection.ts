import { appendAccessSection } from './accessSection.js';
import * as nls from '../../../nls.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { IPermissionService } from '../../services/permissions/permissionService.js';
import { SettingsStateObserver } from './settingsStateObserver.js';
import type { SystemPolicy, SystemSupport } from '../../../platform/engine/common/engine.js';
import { IAccessService, AccessReadbackError, type LocalSecurity } from '../../services/access/accessService.js';
import { AccessToggle } from './accessToggle.js';

export class LocalSecuritySection extends Disposable {
	public readonly element = document.createElement('section');
	private readonly status = document.createElement('p');
	private readonly rows = new Map<keyof SystemPolicy, AccessToggle>();
	private current: LocalSecurity | undefined;
	private busy = false;
	private reading = false;
	private revision = 0;
	private readonly definitions: readonly (readonly [keyof SystemPolicy, keyof SystemSupport, string, string])[] = [
		['allow_lock', 'lock', nls.localize('security.lock', 'Allow remote lock'), nls.localize('security.lockHelp', 'Let permitted devices lock this computer.')],
		['allow_restart', 'restart', nls.localize('security.restart', 'Allow remote restart'), nls.localize('security.restartHelp', 'Let permitted devices restart this computer.')],
		['allow_shutdown', 'shutdown', nls.localize('security.shutdown', 'Allow remote shutdown'), nls.localize('security.shutdownHelp', 'Let permitted devices shut down this computer.')],
		['allow_privacy', 'privacy', nls.localize('security.privacy', 'Allow privacy mode'), nls.localize('security.privacyHelp', 'Let permitted devices request privacy mode during remote sessions.')],
	];

	constructor(@IAccessService private readonly access: IAccessService, @IPermissionService permissions: IPermissionService) {
		super();
		this.element.className = 'access-section';
		const header = document.createElement('div');
		header.className = 'access-section-header';
		const heading = document.createElement('h2');
		heading.textContent = nls.localize('security.local', 'Remote action rules');
		header.append(heading);
		const help = document.createElement('p');
		help.className = 'access-description';
		help.textContent = nls.localize('security.policyHelp', 'Allow these requests from paired devices when System actions sharing is enabled. These switches set VniLink’s rules; they do not run an action or grant operating-system access.');
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
		this._register(new SettingsStateObserver(this.element, permissions.onDidFocusWindow, () => this.load()));
		if (access.available) {
			void this.load();
		} else {
			this.status.textContent = nls.localize('security.unavailable', 'Security policy is unavailable while the connection engine is stopped.');
		}
	}

	private disposed(): boolean {
		return this._store.isDisposed;
	}

	private setBusy(busy: boolean): void {
		this.busy = busy;
		this.element.setAttribute('aria-busy', String(busy));
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
		if (this.busy || this.reading || this.disposed()) {
			return;
		}
		const revision = this.revision;
		this.reading = true;
		if (!this.current) {
			this.setBusy(true);
		}
		try {
			const state = await this.access.loadPolicy();
			if (!this.disposed() && revision === this.revision) {
				this.render(state);
				this.status.textContent = '';
			}
		} catch {
			if (!this.disposed() && revision === this.revision) {
				this.current = undefined;
				this.status.textContent = nls.localize('security.loadFailed', 'Could not load security policy. The policy will be checked again automatically.');
			}
		} finally {
			this.reading = false;
			if (!this.disposed() && revision === this.revision) {
				this.setBusy(false);
			}
		}
	}

	private async change(key: keyof SystemPolicy, allowed: boolean): Promise<void> {
		const capability = this.definitions.find(([policy]) => policy === key)?.[1];
		if (this.busy || !this.current || !capability || !this.current.support[capability] || this.disposed()) {
			return;
		}
		this.revision++;
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
					this.status.textContent = nls.localize('security.readbackFailed', 'Policy changed, but could not be reloaded. Checking the current policy before enabling controls.');
				} else {
					this.render(previous);
					this.current = undefined;
					this.status.textContent = nls.localize('security.changeFailed', 'Could not confirm the change. Checking the current policy before enabling controls.');
				}
			}
		} finally {
			if (!this.disposed()) {
				this.setBusy(false);
			}
		}
	}
}

import * as nls from '../../../nls.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { SharingPolicy } from '../../../platform/engine/common/engine.js';
import { IAccessService, AccessReadbackError, AccessWriteError } from '../../services/access/accessService.js';
import { IPermissionService } from '../../services/permissions/permissionService.js';
import { AccessToggle } from './accessToggle.js';
import { appendAccessSection } from './accessSection.js';
import { SettingsStateObserver } from './settingsStateObserver.js';

/** One installation's consent, shared by every remembered paired computer. */
export class SharingSection extends Disposable {
	public readonly element = document.createElement('section');
	private readonly status = document.createElement('p');
	private readonly rows = new Map<keyof SharingPolicy, AccessToggle>();
	private current: Readonly<SharingPolicy> | undefined;
	private busy = false;
	private reading = false;
	private revision = 0;

	constructor(@IAccessService private readonly access: IAccessService, @IPermissionService permissions: IPermissionService) {
		super();
		this.element.className = 'access-section';
		const heading = document.createElement('h2');
		heading.textContent = nls.localize('sharing.title', 'Sharing with paired computers');
		const help = document.createElement('p');
		help.className = 'access-description';
		help.textContent = nls.localize('sharing.description', 'These settings apply to every paired computer. Turning a service off stops its active use. System permissions still apply.');
		const body = document.createElement('div');
		appendAccessSection(this.element, heading, help, [body]);
		const definitions: readonly (readonly [keyof SharingPolicy, string, string])[] = [
			['view', nls.localize('sharing.screen', 'Screen viewing and remote control'), nls.localize('sharing.screenHelp', 'Allow paired computers to view this screen and use the keyboard and mouse.')],
			['files', nls.localize('sharing.files', 'Files'), nls.localize('sharing.filesHelp', 'Allow file browsing and transfers on this computer.')],
			['terminal', nls.localize('sharing.terminal', 'Terminal'), nls.localize('sharing.terminalHelp', 'Allow shell sessions on this computer.')],
			['camera', nls.localize('sharing.camera', 'Camera'), nls.localize('sharing.cameraHelp', 'Allow access to the camera sources you share.')],
			['microphone', nls.localize('sharing.microphone', 'Microphone'), nls.localize('sharing.microphoneHelp', 'Allow listening to shared microphone input.')],
			['system_audio', nls.localize('sharing.audio', 'System audio'), nls.localize('sharing.audioHelp', 'Allow listening to audio played by this computer.')],
			['clipboard', nls.localize('sharing.clipboard', 'Clipboard'), nls.localize('sharing.clipboardHelp', 'Allow clipboard exchange during desktop sessions.')],
			['recording', nls.localize('sharing.recording', 'Recording'), nls.localize('sharing.recordingHelp', 'Allow recording through VniLink during permitted desktop sessions.')],
			['system_actions', nls.localize('sharing.actions', 'System actions'), nls.localize('sharing.actionsHelp', 'Allow the supported actions enabled under Remote action rules.')],
		];
		for (const [key, title, description] of definitions) {
			const row = this._register(new AccessToggle(title, description, allowed => {
				void this.change(key, allowed);
			}));
			row.input.dataset.sharing = key;
			this.rows.set(key, row);
			body.append(row.element);
		}
		this.status.className = 'access-status';
		this.status.setAttribute('role', 'status');
		body.append(this.status);
		this._register(new SettingsStateObserver(this.element, permissions.onDidFocusWindow, () => this.load()));
		this.enable();
		void this.load();
	}

	private enable(): void {
		for (const row of this.rows.values()) {
			row.setEnabled(!this.busy && !!this.current && this.access.available);
		}
	}

	private render(policy: Readonly<SharingPolicy>): void {
		this.current = policy;
		for (const [key, row] of this.rows) {
			row.setValue(policy[key]);
		}
		this.enable();
	}

	private async load(): Promise<void> {
		if (this.busy || this.reading || this.isDisposed()) {
			return;
		}
		this.reading = true;
		const revision = this.revision;
		try {
			const policy = await this.access.loadSharing();
			if (!this.isDisposed() && revision === this.revision) {
				this.render(policy);
				this.status.textContent = '';
			}
		} catch {
			if (!this.isDisposed() && revision === this.revision) {
				this.status.textContent = nls.localize('sharing.readFailed', 'Sharing settings could not be loaded.');
			}
		} finally {
			this.reading = false;
		}
	}

	private async change(key: keyof SharingPolicy, allowed: boolean): Promise<void> {
		if (!this.current || this.busy || this.isDisposed()) {
			return;
		}
		this.busy = true;
		this.revision++;
		this.enable();
		try {
			const policy = await this.access.setSharing(key === 'view' ? { view: allowed, control: allowed } : { [key]: allowed });
			if (!this.isDisposed()) {
				this.render(policy);
				this.status.textContent = '';
			}
		} catch (error) {
			if (!this.isDisposed()) {
				if (error instanceof AccessWriteError) {
					this.current = error.policy;
					if (error.policy) {
						this.render(error.policy);
					}
				} else if (error instanceof AccessReadbackError) {
					this.current = undefined;
				} else {
					this.render(this.current);
				}
				if (error instanceof AccessReadbackError) {
					this.status.textContent = nls.localize('sharing.uncertain', 'The change was saved, but its current state could not be read. Waiting for updated settings.');
				} else if (error instanceof AccessWriteError) {
					this.status.textContent = error.policy
						? nls.localize('sharing.incomplete', 'The operation could not be completed. Current sharing settings are shown.')
						: nls.localize('sharing.unknownWrite', 'The operation could not be completed, and current sharing settings could not be read.');
				} else {
					this.status.textContent = nls.localize('sharing.writeFailed', 'Sharing settings could not be changed.');
				}
			}
		} finally {
			this.busy = false;
			if (!this.isDisposed()) {
				this.enable();
			}
		}
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}

}

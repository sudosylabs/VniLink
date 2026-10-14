import { appendAccessSection } from './accessSection.js';
import { wrapSelect } from './settingsControls.js';
import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import { IAccessService, AccessReadbackError, type AccessPermission, type PeerAccess } from '../../../services/access/common/accessService.js';
import { AccessToggle } from './accessToggle.js';

export class PeerAccessSection extends Disposable {
	public readonly element = document.createElement('section');
	private readonly select = document.createElement('select');
	private readonly refreshButton = document.createElement('button');
	private readonly identity = document.createElement('code');
	private readonly fields = document.createElement('div');
	private readonly status = document.createElement('p');
	private readonly rows = new Map<AccessPermission, AccessToggle>();
	private current: PeerAccess | undefined;
	private busy = false;
	private revision = 0;

	constructor(@IAccessService private readonly access: IAccessService) {
		super();
		this.element.className = 'access-section';
		const header = document.createElement('div');
		header.className = 'access-section-header';
		const heading = document.createElement('h2');
		heading.textContent = nls.localize('access.devices', 'Device access');
		this.refreshButton.type = 'button';
		this.refreshButton.className = 'access-text-button';
		this.refreshButton.textContent = nls.localize('access.refresh', 'Refresh');
		header.append(heading, this.refreshButton);
		const help = document.createElement('p');
		help.className = 'access-description';
		help.textContent = nls.localize('access.direction', 'Choose what a paired device can access on this computer.');
		const selector = document.createElement('label');
		selector.className = 'access-device-selector';
		const title = document.createElement('span');
		title.textContent = nls.localize('access.device', 'Device');
		selector.append(title, wrapSelect(this.select));
		this.identity.className = 'access-identity';
		this.fields.className = 'access-permissions';
		this.fields.hidden = true;
		this.status.className = 'access-status';
		this.status.setAttribute('role', 'status');
		const definitions: readonly (readonly [AccessPermission, string, string])[] = [
			['view', nls.localize('access.view', 'View screen'), nls.localize('access.viewHelp', 'See the screen of this computer.')],
			['control', nls.localize('access.control', 'Keyboard & mouse'), nls.localize('access.controlHelp', 'Control this computer’s keyboard and pointer.')],
			['clipboard', nls.localize('access.clipboard', 'Clipboard'), nls.localize('access.clipboardHelp', 'Exchange clipboard contents.')],
			['files', nls.localize('access.files', 'Files'), nls.localize('access.filesHelp', 'Transfer files through approved folders.')],
			['terminal', nls.localize('access.terminal', 'Terminal'), nls.localize('access.terminalHelp', 'Open remote shell sessions on this computer.')],
			['camera', nls.localize('access.camera', 'Camera'), nls.localize('access.cameraHelp', 'Access camera streams from this computer.')],
			['systemAudio', nls.localize('access.systemAudio', 'System audio'), nls.localize('access.systemAudioHelp', 'Listen to audio played by this computer.')],
			['microphone', nls.localize('access.microphone', 'Microphone'), nls.localize('access.microphoneHelp', 'Listen to microphone input from this computer.')],
			['recording', nls.localize('access.recording', 'Recording'), nls.localize('access.recordingHelp', 'Record sessions this device is permitted to access.')],
			['systemActions', nls.localize('access.systemActions', 'System actions'), nls.localize('access.systemActionsHelp', 'Request actions permitted by this computer’s policy below.')],
		];
		for (const [key, title, description] of definitions) {
			const row = this._register(new AccessToggle(title, description, allowed => {
				void this.change(key, allowed);
			}));
			row.input.dataset.permission = key;
			this.rows.set(key, row);
			this.fields.append(row.element);
		}
		const note = document.createElement('p');
		note.className = 'access-description';
		note.textContent = nls.localize('access.permissionNote', 'Changes apply immediately. Removing permission can stop active work. Device support and operating-system consent still apply.');
		this.fields.append(note);
		appendAccessSection(this.element, header, help, [selector, this.identity, this.fields, this.status]);
		this._register(addDisposableListener(this.refreshButton, 'click', () => {
			void this.refresh();
		}));
		this._register(addDisposableListener(this.select, 'change', () => {
			if (!this.busy) {
				void this.load(this.select.value);
			}
		}));
		if (access.available) {
			void this.refresh();
		} else {
			this.select.disabled = true;
			this.refreshButton.disabled = true;
			this.status.textContent = nls.localize('access.unavailable', 'The connection engine is unavailable. Restart VniLink to try again.');
		}
	}

	private disposed(): boolean {
		return this._store.isDisposed;
	}

	private setBusy(busy: boolean): void {
		this.busy = busy;
		this.element.setAttribute('aria-busy', String(busy));
		this.select.disabled = busy;
		this.refreshButton.disabled = busy;
		for (const row of this.rows.values()) {
			row.setEnabled(!busy && !!this.current);
		}
	}

	private render(peer: PeerAccess): void {
		this.current = peer;
		this.identity.textContent = peer.device.id;
		this.fields.hidden = false;
		for (const [key, row] of this.rows) {
			row.setValue(peer.permissions[key]);
		}
	}

	private async refresh(): Promise<void> {
		if (this.busy || this.disposed()) {
			return;
		}
		this.setBusy(true);
		this.status.textContent = nls.localize('access.loadingDevices', 'Loading paired devices…');
		const selected = this.current?.device.id;
		try {
			const devices = await this.access.listDevices();
			if (this.disposed()) {
				return;
			}
			this.select.replaceChildren();
			for (const device of devices) {
				const option = document.createElement('option');
				option.value = device.id;
				option.textContent = `${device.name} · ${device.id.slice(0, 8)}`;
				this.select.append(option);
			}
			this.setBusy(false);
			if (!devices.length) {
				this.current = undefined;
				this.fields.hidden = true;
				this.identity.textContent = '';
				this.select.disabled = true;
				this.status.textContent = nls.localize('access.noDevices', 'No paired devices. Pair a device to manage its access here.');
				return;
			}
			this.select.value = devices.some(device => device.id === selected) && selected ? selected : devices[0].id;
			await this.load(this.select.value);
		} catch {
			if (!this.disposed()) {
				this.current = undefined;
				this.fields.hidden = true;
				this.identity.textContent = '';
				this.status.textContent = nls.localize('access.deviceLoadFailed', 'Could not load paired devices. Refresh to try again.');
				this.setBusy(false);
			}
		}
	}

	private async load(id: string): Promise<void> {
		if (this.disposed()) {
			return;
		}
		const revision = ++this.revision;
		this.current = undefined;
		this.fields.hidden = true;
		this.identity.textContent = '';
		this.setBusy(true);
		try {
			const peer = await this.access.loadPeer(id);
			if (this.disposed() || revision !== this.revision) {
				return;
			}
			this.render(peer);
			this.status.textContent = '';
		} catch {
			if (!this.disposed() && revision === this.revision) {
				this.status.textContent = nls.localize('access.peerLoadFailed', 'Could not load this device’s permissions. Refresh to try again.');
			}
		} finally {
			if (!this.disposed() && revision === this.revision) {
				this.setBusy(false);
			}
		}
	}

	private async change(key: AccessPermission, allowed: boolean): Promise<void> {
		if (this.busy || !this.current || this.disposed()) {
			return;
		}
		const previous = this.current;
		this.setBusy(true);
		this.status.textContent = '';
		try {
			const peer = await this.access.setPermission(previous.device.id, key, allowed);
			if (!this.disposed()) {
				this.render(peer);
			}
		} catch (error) {
			if (!this.disposed()) {
				if (error instanceof AccessReadbackError) {
					this.current = undefined;
					this.fields.hidden = true;
					this.status.textContent = nls.localize('access.readbackFailed', 'Change applied, but permissions could not be reloaded. Refresh before editing again.');
				} else {
					this.render(previous);
					this.current = undefined;
					this.fields.hidden = true;
					this.status.textContent = nls.localize('access.changeFailed', 'Could not confirm the change. Refresh to check this device’s current permissions.');
				}
			}
		} finally {
			if (!this.disposed()) {
				this.setBusy(false);
			}
		}
	}
}

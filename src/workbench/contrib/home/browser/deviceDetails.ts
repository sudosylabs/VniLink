import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { Emitter } from '../../../../base/common/event.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import { renderIcon } from '../../../../base/browser/ui/icon/icon.js';
import { Icons } from '../../../../base/common/icons.js';
import type { DeviceSummary, IDeviceService } from '../../../services/devices/common/deviceService.js';
import { devicePlatform, devicePresence } from './deviceLabels.js';

/** Owns one selection and fences late command feedback when selection changes. */
export class DeviceDetails extends Disposable {
	public readonly element = document.createElement('aside');
	private readonly closed = this._register(new Emitter<void>());
	public readonly onDidClose = this.closed.event;
	private readonly title = document.createElement('h2');
	private readonly state = document.createElement('p');
	private readonly identity = document.createElement('code');
	private readonly connect = document.createElement('button');
	private readonly name = document.createElement('input');
	private readonly rename = document.createElement('button');
	private readonly forget = document.createElement('button');
	private readonly confirm = document.createElement('div');
	private readonly status = document.createElement('p');
	private device: DeviceSummary | undefined;
	private pending = new Set<string>();

	constructor(id: string, private readonly devices: IDeviceService) {
		super();
		this.element.id = id;
		this.element.className = 'device-panel';
		this.element.hidden = true;
		this.title.id = `${id}-title`;
		this.element.setAttribute('aria-labelledby', this.title.id);
		const header = document.createElement('header');
		const close = document.createElement('button');
		close.type = 'button';
		close.className = 'device-close';
		close.setAttribute('aria-label', nls.localize('devices.close', 'Close device details'));
		close.append(renderIcon(Icons.close));
		header.append(renderIcon(Icons.remoteDesktop, { variant: 'filled' }), close);
		this.state.className = 'device-panel-state';
		this.state.setAttribute('role', 'status');
		this.identity.className = 'device-identity';
		this.connect.type = 'button';
		this.connect.className = 'device-button primary';
		this.connect.textContent = nls.localize('devices.connect', 'Connect');
		const help = document.createElement('p');
		help.className = 'device-help';
		help.textContent = nls.localize('devices.connectionHelp', 'Connect to this computer without starting a remote service. Access is controlled by its owner’s sharing settings.');
		const form = document.createElement('form');
		form.className = 'device-rename';
		const label = document.createElement('label');
		label.textContent = nls.localize('devices.name', 'Display name');
		this.name.type = 'text';
		this.name.maxLength = 128;
		this.name.autocomplete = 'off';
		label.append(this.name);
		this.rename.type = 'submit';
		this.rename.className = 'device-button';
		this.rename.textContent = nls.localize('devices.rename', 'Rename');
		form.append(label, this.rename);
		this.forget.type = 'button';
		this.forget.className = 'device-button device-forget';
		this.forget.textContent = nls.localize('devices.forget', 'Forget device…');
		this.confirm.className = 'device-forget-confirm';
		this.confirm.hidden = true;
		const warning = document.createElement('p');
		warning.textContent = nls.localize('devices.forgetWarning', 'Forget this computer and end its connections? You will need a new invitation to pair again.');
		const cancel = document.createElement('button');
		cancel.type = 'button';
		cancel.className = 'device-button';
		cancel.textContent = nls.localize('devices.keep', 'Keep device');
		const remove = document.createElement('button');
		remove.type = 'button';
		remove.className = 'device-button';
		remove.textContent = nls.localize('devices.confirmForget', 'Forget');
		this.confirm.append(warning, cancel, remove);
		this.status.className = 'device-feedback';
		this.status.setAttribute('role', 'status');
		this.element.append(header, this.title, this.state, this.identity, this.connect, help, form, this.forget, this.confirm, this.status);
		this._register(addDisposableListener(close, 'click', () => {
			this.closed.fire();
		}));
		this._register(addDisposableListener(this.element, 'keydown', event => {
			if ((event as KeyboardEvent).key === 'Escape') {
				event.preventDefault();
				this.closed.fire();
			}
		}));
		this._register(addDisposableListener(this.connect, 'click', () => {
			this.run(id => devices.connect(id));
		}));
		this._register(addDisposableListener(this.name, 'input', () => {
			this.updateControls();
		}));
		this._register(addDisposableListener(form, 'submit', event => {
			event.preventDefault();
			if (!this.rename.disabled) {
				const name = this.name.value.trim();
				this.run(id => devices.rename(id, name));
			}
		}));
		this._register(addDisposableListener(this.forget, 'click', () => {
			this.confirm.hidden = false;
			cancel.focus();
		}));
		this._register(addDisposableListener(cancel, 'click', () => {
			this.confirm.hidden = true;
			this.forget.focus();
		}));
		this._register(addDisposableListener(remove, 'click', () => {
			if (this.device && !this.pending.has(this.device.id)) {
				this.run(id => devices.forget(id));
			}
		}));
	}

	public update(device: DeviceSummary | undefined): void {
		const changed = device?.id !== this.device?.id;
		const clean = this.name.value === this.device?.name;
		this.device = device;
		this.element.hidden = !device;
		if (!device) {
			return;
		}
		if (changed || clean) {
			this.name.value = device.name;
		}
		if (changed) {
			this.status.textContent = '';
			this.confirm.hidden = true;
		}
		this.title.textContent = device.name;
		this.identity.textContent = device.id;
		this.state.textContent = `${devicePlatform(device)} · ${devicePresence(device)}`;
		this.updateControls();
	}

	private updateControls(): void {
		const busy = !!this.device && this.pending.has(this.device.id);
		const enabled = !!this.device && this.devices.available && !busy;
		this.connect.disabled = !enabled || this.device?.presence !== 'disconnected';
		this.connect.textContent = this.device?.presence === 'connected' ? nls.localize('devices.connected', 'Connected') : this.device?.presence === 'connecting' || this.device?.presence === 'reconnecting' ? nls.localize('devices.connecting', 'Connecting…') : nls.localize('devices.connect', 'Connect');
		this.name.disabled = !enabled;
		this.rename.disabled = !enabled || !this.name.value.trim() || this.name.value.trim() === this.device?.name;
		this.forget.disabled = !enabled;
		for (const button of this.confirm.querySelectorAll('button')) {
			button.disabled = !enabled;
		}
		this.element.setAttribute('aria-busy', String(busy));
	}

	private run(action: (id: string) => Promise<void>): void {
		const id = this.device?.id;
		if (!id || this.pending.has(id)) {
			return;
		}
		this.pending.add(id);
		this.status.textContent = '';
		this.updateControls();
		void action(id).catch((error: unknown) => {
			if (!this._store.isDisposed && this.device?.id === id) {
				this.status.textContent = error instanceof Error ? error.message : nls.localize('devices.actionFailed', 'The device action failed. Try again.');
			}
		}).finally(() => {
			this.pending.delete(id);
			if (!this._store.isDisposed) {
				this.updateControls();
			}
		});
	}
}

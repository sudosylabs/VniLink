import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType, h } from '../../../base/browser/dom.js';
import { Button } from '../../../base/browser/ui/button/button.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { InputBox } from '../../../base/browser/ui/inputbox/inputBox.js';
import { Emitter } from '../../../base/common/event.js';
import { Icons } from '../../../base/common/icons.js';
import { KeyCode } from '../../../base/common/keyCodes.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { StandardKeyboardEvent } from '../../../base/browser/keyboardEvent.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { defaultButtonStyles, defaultInputBoxStyles } from '../../../platform/theme/browser/defaultStyles.js';
import { IDeviceService, type DeviceSummary } from '../../services/devices/deviceService.js';
import { devicePlatform, devicePresence } from './deviceLabels.js';
import { DeviceScreenSnapshot } from './deviceScreenSnapshot.js';

/** Longest display name the engine accepts. */
const maxNameLength = 128;

/**
 * Details and commands for one selected computer. A command's late failure
 * is only reported while its computer is still the selection.
 */
export class DeviceDetails extends Disposable {
	public readonly element: HTMLElement;
	private readonly closed = this._register(new Emitter<void>());
	public readonly onDidClose = this.closed.event;
	private readonly title: HTMLElement;
	private readonly state: HTMLElement;
	private readonly identity: HTMLElement;
	private readonly status: HTMLElement;
	private readonly confirm: HTMLElement;
	private readonly connect: Button;
	private readonly name: InputBox;
	private readonly rename: Button;
	private readonly forget: Button;
	private readonly keep: Button;
	private readonly confirmForget: Button;
	private readonly screen: DeviceScreenSnapshot;
	private device: DeviceSummary | undefined;
	private readonly pending = new Set<string>();

	constructor(
		id: string,
		@IDeviceService private readonly devices: IDeviceService,
		@IInstantiationService instantiationService: IInstantiationService,
	) {
		super();
		const panel = h('aside.device-panel', [
			h('header@header'),
			h('h2@title'),
			h('p.device-panel-state@state', { role: 'status' }),
		]);
		this.element = panel.root;
		this.element.id = id;
		this.element.hidden = true;
		this.title = panel.title;
		this.title.id = `${id}-title`;
		this.element.setAttribute('aria-labelledby', this.title.id);
		this.state = panel.state;
		panel.header.append(renderIcon(Icons.remoteDesktop, { variant: 'filled' }));
		const close = append(panel.header, $<HTMLButtonElement>('button.device-close', { type: 'button', 'aria-label': nls.localize('devices.close', 'Close device details') }));
		close.append(renderIcon(Icons.close));
		this.screen = this._register(instantiationService.createInstance(DeviceScreenSnapshot));
		this.element.append(this.screen.element);
		this.identity = append(this.element, $('code.device-identity'));
		this.connect = this.createButton(this.element, nls.localize('devices.connect', 'Connect'), { primary: true });
		append(this.element, $('p.device-help')).textContent = nls.localize('devices.connectionHelp', 'Connect to this computer without starting a remote service. Access is controlled by its owner’s sharing settings.');
		const rename = append(this.element, $('.device-rename'));
		const label = append(rename, $('label'));
		label.append(nls.localize('devices.name', 'Display name'));
		this.name = this._register(new InputBox(label, undefined, { inputBoxStyles: defaultInputBoxStyles }));
		this.name.inputElement.maxLength = maxNameLength;
		this.name.inputElement.autocomplete = 'off';
		this.rename = this.createButton(rename, nls.localize('devices.rename', 'Rename'));
		this.forget = this.createButton(this.element, nls.localize('devices.forget', 'Forget device…'), { className: 'device-forget' });
		this.confirm = append(this.element, $('.device-forget-confirm'));
		this.confirm.hidden = true;
		append(this.confirm, $('p')).textContent = nls.localize('devices.forgetWarning', 'Forget this computer and end its connections? You will need a new invitation to pair again.');
		this.keep = this.createButton(this.confirm, nls.localize('devices.keep', 'Keep device'));
		this.confirmForget = this.createButton(this.confirm, nls.localize('devices.confirmForget', 'Forget'));
		this.status = append(this.element, $('p.device-feedback', { role: 'status' }));
		this._register(addDisposableListener(close, EventType.CLICK, () => {
			this.closed.fire();
		}));
		this._register(addDisposableListener(this.element, EventType.KEY_DOWN, event => {
			if (event.key === 'Escape') {
				event.preventDefault();
				this.closed.fire();
			}
		}));
		this._register(this.connect.onDidClick(() => {
			this.run(target => this.devices.connect(target));
		}));
		this._register(this.name.onDidChange(() => {
			this.updateControls();
		}));
		this._register(addDisposableListener(this.name.inputElement, EventType.KEY_DOWN, event => {
			if (new StandardKeyboardEvent(event).equals(KeyCode.Enter)) {
				event.preventDefault();
				this.submitName();
			}
		}));
		this._register(this.rename.onDidClick(() => {
			this.submitName();
		}));
		this._register(this.forget.onDidClick(() => {
			this.confirm.hidden = false;
			this.keep.focus();
		}));
		this._register(this.keep.onDidClick(() => {
			this.confirm.hidden = true;
			this.forget.focus();
		}));
		this._register(this.confirmForget.onDidClick(() => {
			this.run(target => this.devices.forget(target));
		}));
	}

	private createButton(container: HTMLElement, label: string, options: { primary?: boolean; className?: string } = {}): Button {
		const button = this._register(new Button(container, { ...defaultButtonStyles, secondary: !options.primary, title: false }));
		button.label = label;
		button.element.classList.add('device-button', ...(options.primary ? ['primary'] : []), ...(options.className ? [options.className] : []));
		return button;
	}

	/** Shows `device`, keeping an unsaved name edit while the same computer stays selected. */
	public update(device: DeviceSummary | undefined): void {
		const changed = device?.id !== this.device?.id;
		const clean = this.name.value === this.device?.name;
		this.device = device;
		this.screen.show(device);
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

	private submitName(): void {
		if (this.rename.enabled) {
			const name = this.name.value.trim();
			this.run(target => this.devices.rename(target, name));
		}
	}

	private updateControls(): void {
		const device = this.device;
		const busy = !!device && this.pending.has(device.id);
		const enabled = !!device && this.devices.available && !busy;
		const name = this.name.value.trim();
		this.connect.enabled = enabled && device.presence === 'disconnected';
		this.connect.label = device?.presence === 'connected' ? nls.localize('devices.connected', 'Connected')
			: device?.presence === 'connecting' || device?.presence === 'reconnecting' ? nls.localize('devices.connecting', 'Connecting…')
				: nls.localize('devices.connect', 'Connect');
		if (enabled) {
			this.name.enable();
		} else {
			this.name.disable();
		}
		this.rename.enabled = enabled && name.length > 0 && name !== device.name;
		this.forget.enabled = enabled;
		this.keep.enabled = enabled;
		this.confirmForget.enabled = enabled;
		this.element.setAttribute('aria-busy', String(busy));
	}

	/** Runs a command for the selected computer; its failure is shown only while it stays selected. */
	private run(command: (id: string) => Promise<void>): void {
		const id = this.device?.id;
		if (!id || this.pending.has(id)) {
			return;
		}
		this.pending.add(id);
		this.status.textContent = '';
		this.updateControls();
		command(id).catch((error: unknown) => {
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

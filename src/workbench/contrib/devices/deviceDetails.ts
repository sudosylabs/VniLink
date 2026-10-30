// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType, h } from '../../../base/browser/dom.js';
import { StandardKeyboardEvent } from '../../../base/browser/keyboardEvent.js';
import { Button } from '../../../base/browser/ui/button/button.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { InputBox } from '../../../base/browser/ui/inputbox/inputBox.js';
import { Emitter } from '../../../base/common/event.js';
import { Icons, type IconId } from '../../../base/common/icons.js';
import { KeyCode } from '../../../base/common/keyCodes.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { RemoteCapability, RemoteCapabilityStatus, RemoteStatus } from '../../../platform/engine/common/engine.js';
import { ICommandService } from '../../../platform/commands/common/commands.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { defaultButtonStyles, defaultInputBoxStyles } from '../../../platform/theme/browser/defaultStyles.js';
import { IDeviceService, type DeviceSummary } from '../../services/devices/deviceService.js';
import { ISystemActionService } from '../../services/systemActions/systemActionService.js';
import { devicePlatform, devicePresence } from './deviceLabels.js';
import { DeviceScreenSnapshot } from './deviceScreenSnapshot.js';

/** Longest display name the engine accepts. */
const maxNameLength = 128;

/** A service row: what it offers, the capabilities the remote computer reports for it, and the command that opens it. */
interface ServiceRow {
	readonly label: string;
	readonly icon: IconId;
	readonly capabilities: readonly RemoteCapability[];
	/** Run with the device id while the row is available; rows without one only report. */
	readonly command?: string;
	/** A system action row also shows how the action it asked for stands, and can cancel it. */
	readonly systemAction?: boolean;
}

const serviceRows: readonly ServiceRow[] = [
	{ label: nls.localize('devices.serviceDesktop', 'Remote desktop'), icon: Icons.remoteDesktop, capabilities: ['desktopControl'], command: 'vnilink.desktop.open' },
	{ label: nls.localize('devices.serviceView', 'View screen only'), icon: Icons.view, capabilities: ['desktopView'], command: 'vnilink.desktop.view' },
	{ label: nls.localize('devices.serviceTerminal', 'Terminal'), icon: Icons.terminal, capabilities: ['terminal'], command: 'vnilink.terminal.open' },
	{ label: nls.localize('devices.serviceFiles', 'Files'), icon: Icons.files, capabilities: ['files'], command: 'vnilink.files.open' },
	{ label: nls.localize('devices.serviceCamera', 'Camera'), icon: Icons.camera, capabilities: ['camera'], command: 'vnilink.camera.open' },
	{ label: nls.localize('devices.serviceLock', 'Lock screen'), icon: Icons.lock, capabilities: ['systemLock'], command: 'vnilink.system.lock', systemAction: true },
	{ label: nls.localize('devices.servicePower', 'Restart or shut down'), icon: Icons.power, capabilities: ['systemRestart', 'systemShutdown'], command: 'vnilink.system.restartOrShutDown', systemAction: true },
];

function remoteStatusLabel(status: RemoteStatus): string {
	switch (status) {
		case 'available': return nls.localize('devices.statusAvailable', 'Available');
		case 'turnedOff': return nls.localize('devices.statusTurnedOff', 'Turned off by its owner');
		case 'needsPermission': return nls.localize('devices.statusPermission', 'Needs permission on that computer');
		case 'unsupported': return nls.localize('devices.statusUnsupported', 'Not supported there');
		case 'unavailable': return nls.localize('devices.statusUnavailable', 'Unavailable right now');
	}
}

/** A row is available when any of its capabilities is; otherwise it reports its first capability's reason. */
function rowStatus(row: ServiceRow, report: readonly RemoteCapabilityStatus[]): RemoteStatus {
	const statuses = row.capabilities.map(capability => report.find(entry => entry.capability === capability)?.status ?? 'unsupported');
	return statuses.includes('available') ? 'available' : statuses[0];
}

/** Shows the start and end of a long id; the full id stays in the title. */
function shortId(id: string): string {
	return id.length > 16 ? `${id.slice(0, 8)}…${id.slice(-6)}` : id;
}

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
	private readonly renameForm: HTMLElement;
	private readonly name: InputBox;
	private readonly save: Button;
	private readonly services: {
		readonly row: ServiceRow;
		readonly element: HTMLElement;
		readonly status: HTMLElement;
		readonly open?: HTMLButtonElement;
		/** Cancels a restart or shutdown counting down; system action rows only. */
		readonly cancel?: HTMLButtonElement;
	}[] = [];
	private readonly idValue: HTMLElement;
	private readonly copyId: HTMLButtonElement;
	private readonly hostname: HTMLElement;
	private readonly system: HTMLElement;
	private readonly addresses: HTMLElement;
	private readonly renameButton: HTMLButtonElement;
	private readonly forget: HTMLButtonElement;
	private readonly confirm: HTMLElement;
	private readonly keep: Button;
	private readonly confirmForget: Button;
	private readonly status: HTMLElement;
	private readonly screen: DeviceScreenSnapshot;
	private device: DeviceSummary | undefined;
	private report: readonly RemoteCapabilityStatus[] | null = null;
	/** Bumped per availability request, so only the newest answer for the selection lands. */
	private reportRevision = 0;
	private readonly pending = new Set<string>();

	constructor(
		id: string,
		@IDeviceService private readonly devices: IDeviceService,
		@IInstantiationService instantiationService: IInstantiationService,
		@ICommandService private readonly commandService: ICommandService,
		@ISystemActionService private readonly systemActions: ISystemActionService,
	) {
		super();
		const closeLabel = nls.localize('devices.close', 'Close device details');
		const copyLabel = nls.localize('devices.copyId', 'Copy device ID');
		const panel = h('aside.device-panel', [
			h('div.device-panel-top', [h('button.device-icon-button.device-close@close', { type: 'button', title: closeLabel, 'aria-label': closeLabel })]),
			h('div.device-panel-screen@screen'),
			h('header.device-panel-identity', [
				h('h2@title'),
				h('div.device-rename@renameForm'),
				h('p.device-panel-state@state', { role: 'status' }),
			]),
			h('section.device-section@servicesSection', [
				h('ul.device-services@services', { role: 'list' }),
			]),
			h('section.device-section@factsSection', [
				h('dl.device-facts', [
					h('div', [h('dt@idLabel'), h('dd', [h('code.device-id@idValue'), h('button.device-icon-button.device-copy@copyId', { type: 'button', title: copyLabel, 'aria-label': copyLabel })])]),
					h('div', [h('dt@hostnameLabel'), h('dd@hostname')]),
					h('div', [h('dt@systemLabel'), h('dd@system')]),
					h('div', [h('dt@addressesLabel'), h('dd@addresses')]),
				]),
			]),
			h('p.device-section.device-access-note@accessNote'),
			h('section.device-section@manageSection', [
				h('button.device-text-button@renameButton', { type: 'button' }),
				h('button.device-text-button.device-forget@forget', { type: 'button' }),
				h('div.device-forget-confirm@confirm', [h('p@forgetWarning')]),
			]),
			h('p.device-feedback@status', { role: 'status' }),
		]);
		// Hairlines separate the groups, as in a native inspector; the names are for assistive technology.
		panel.servicesSection.setAttribute('aria-label', nls.localize('devices.services', 'Services'));
		panel.factsSection.setAttribute('aria-label', nls.localize('devices.details', 'Details'));
		panel.manageSection.setAttribute('aria-label', nls.localize('devices.manage', 'Manage'));
		panel.idLabel.textContent = nls.localize('devices.id', 'Device ID');
		panel.hostnameLabel.textContent = nls.localize('devices.hostname', 'Computer');
		panel.systemLabel.textContent = nls.localize('devices.system', 'System');
		panel.addressesLabel.textContent = nls.localize('devices.addresses', 'Addresses');
		panel.accessNote.textContent = nls.localize('devices.accessNote', 'What paired computers may use on this computer is set in Access & Security.');
		panel.renameButton.textContent = nls.localize('devices.renameAction', 'Rename…');
		panel.forget.textContent = nls.localize('devices.forget', 'Forget device…');
		panel.forgetWarning.textContent = nls.localize('devices.forgetWarning', 'Forget this computer and end its connections? You will need a new invitation to pair again.');
		this.element = panel.root;
		this.element.id = id;
		this.element.hidden = true;
		this.title = panel.title;
		this.title.id = `${id}-title`;
		this.element.setAttribute('aria-labelledby', this.title.id);
		this.state = panel.state;
		panel.close.append(renderIcon(Icons.close));
		this.screen = this._register(instantiationService.createInstance(DeviceScreenSnapshot));
		panel.screen.append(this.screen.element);
		this.renameForm = panel.renameForm;
		this.renameForm.hidden = true;
		this.name = this._register(new InputBox(this.renameForm, undefined, { inputBoxStyles: defaultInputBoxStyles, ariaLabel: nls.localize('devices.name', 'Display name') }));
		this.name.inputElement.maxLength = maxNameLength;
		this.name.inputElement.autocomplete = 'off';
		this.save = this.createButton(this.renameForm, nls.localize('devices.saveName', 'Save'), true);
		for (const row of serviceRows) {
			const label = $('span.device-service-label');
			const status = $('span.device-service-status');
			label.textContent = row.label;
			// A row that opens something is a button inside its list item; the others only report.
			const command = row.command;
			const open = command ? $<HTMLButtonElement>('button.device-service', { type: 'button' }) : undefined;
			const element = open ?? $('li.device-service');
			append(element, renderIcon(row.icon), label, status);
			const item = open ? append(panel.services, $('li.device-service-item')) : panel.services;
			append(item, element);
			if (open && command) {
				this._register(addDisposableListener(open, EventType.CLICK, () => {
					const deviceId = this.device?.id;
					if (deviceId) {
						void this.commandService.executeCommand(command, deviceId);
					}
				}));
			}
			// A restart or shutdown counting down can be cancelled from its row.
			const cancel = row.systemAction ? append(item, $<HTMLButtonElement>('button.device-service-cancel', { type: 'button' })) : undefined;
			if (cancel) {
				cancel.textContent = nls.localize('devices.cancelSystemAction', 'Cancel');
				cancel.hidden = true;
				this._register(addDisposableListener(cancel, EventType.CLICK, () => {
					const deviceId = this.device?.id;
					if (deviceId) {
						void this.commandService.executeCommand('vnilink.system.cancel', deviceId);
					}
				}));
			}
			this.services.push({ row, element, status, open, cancel });
		}
		this.idValue = panel.idValue;
		this.copyId = panel.copyId;
		this.copyId.append(renderIcon(Icons.copy));
		this.hostname = panel.hostname;
		this.system = panel.system;
		this.addresses = panel.addresses;
		this.renameButton = panel.renameButton;
		this.forget = panel.forget;
		this.confirm = panel.confirm;
		this.confirm.hidden = true;
		this.keep = this.createButton(this.confirm, nls.localize('devices.keep', 'Keep device'));
		this.confirmForget = this.createButton(this.confirm, nls.localize('devices.confirmForget', 'Forget'));
		this.status = panel.status;
		this.registerListeners(panel.close);
	}

	private createButton(container: HTMLElement, label: string, primary = false): Button {
		const button = this._register(new Button(container, { ...defaultButtonStyles, secondary: !primary, title: false }));
		button.label = label;
		button.element.classList.add('device-button', ...(primary ? ['primary'] : []));
		return button;
	}

	private registerListeners(close: HTMLElement): void {
		this._register(this.systemActions.onDidChange(deviceId => {
			if (deviceId === this.device?.id) {
				this.renderServices();
			}
		}));
		this._register(addDisposableListener(close, EventType.CLICK, () => {
			this.closed.fire();
		}));
		this._register(addDisposableListener(this.element, EventType.KEY_DOWN, event => {
			if (event.key === 'Escape') {
				event.preventDefault();
				this.closed.fire();
			}
		}));
		this._register(this.devices.onDidChangeAvailability(id => {
			if (id === this.device?.id) {
				void this.loadAvailability(id);
			}
		}));
		this._register(addDisposableListener(this.copyId, EventType.CLICK, () => {
			void this.copyDeviceId();
		}));
		this._register(addDisposableListener(this.renameButton, EventType.CLICK, () => {
			this.setRenaming(true);
		}));
		this._register(this.name.onDidChange(() => {
			this.updateControls();
		}));
		this._register(addDisposableListener(this.name.inputElement, EventType.KEY_DOWN, event => {
			const key = new StandardKeyboardEvent(event);
			if (key.equals(KeyCode.Enter)) {
				event.preventDefault();
				this.submitName();
			} else if (key.equals(KeyCode.Escape)) {
				// Escape leaves the field, not the panel.
				event.preventDefault();
				event.stopPropagation();
				this.setRenaming(false);
			}
		}));
		this._register(this.save.onDidClick(() => {
			this.submitName();
		}));
		this._register(addDisposableListener(this.forget, EventType.CLICK, () => {
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

	/** Shows `device`, keeping an open rename while the same computer stays selected. */
	public update(device: DeviceSummary | undefined): void {
		const previous = this.device;
		const changed = device?.id !== previous?.id;
		this.device = device;
		this.screen.show(device);
		this.element.hidden = !device;
		if (!device) {
			this.reportRevision++;
			this.report = null;
			return;
		}
		if (changed) {
			this.status.textContent = '';
			this.confirm.hidden = true;
			this.setRenaming(false);
			this.report = null;
		}
		if (changed || previous?.presence !== device.presence) {
			void this.loadAvailability(device.id);
		}
		this.title.textContent = device.name;
		this.state.textContent = nls.localize('devices.panelState', '{0} · {1}', devicePresence(device), devicePlatform(device));
		this.state.dataset.presence = device.presence;
		this.idValue.textContent = shortId(device.id);
		this.idValue.title = device.id;
		this.hostname.textContent = device.hostname ?? nls.localize('devices.hostnameUnknown', 'Not reported');
		this.system.textContent = devicePlatform(device);
		this.addresses.textContent = nls.localize('devices.addressCounts', '{0} direct, {1} relay', device.direct, device.relays);
		this.renderServices();
		this.updateControls();
	}

	private async loadAvailability(id: string): Promise<void> {
		const revision = ++this.reportRevision;
		let report: readonly RemoteCapabilityStatus[] | null;
		try {
			report = await this.devices.availability(id);
		} catch {
			report = null;
		}
		// The selection may have moved on while the engine answered.
		if (this.isDisposed() || revision !== this.reportRevision || this.device?.id !== id) {
			return;
		}
		this.report = report;
		this.renderServices();
	}

	private renderServices(): void {
		const device = this.device;
		if (!device) {
			return;
		}
		const connected = device.presence === 'connected';
		const progress = this.systemActions.progress(device.id);
		for (const { row, element, status, open, cancel } of this.services) {
			// Away from the computer, every row says why instead of a stale report.
			const value = !connected ? undefined : this.report ? rowStatus(row, this.report) : null;
			status.textContent = value === undefined ? devicePresence(device)
				: value === null ? nls.localize('devices.statusChecking', 'Checking…')
					: remoteStatusLabel(value);
			element.dataset.status = value ?? 'none';
			element.setAttribute('aria-disabled', String(value !== 'available'));
			if (open) {
				open.disabled = value !== 'available';
			}
			// The action this row asked for says how it stands, in place of availability.
			const own = row.systemAction && progress && (progress.action === 'lock') === row.capabilities.includes('systemLock') ? progress : undefined;
			if (own) {
				status.textContent = own.message;
				if (open) {
					open.disabled ||= own.phase !== 'ended';
				}
			}
			if (cancel) {
				cancel.hidden = own?.phase !== 'scheduled';
			}
		}
	}

	private async copyDeviceId(): Promise<void> {
		const id = this.device?.id;
		if (!id) {
			return;
		}
		try {
			await navigator.clipboard.writeText(id);
			if (!this.isDisposed() && this.device?.id === id) {
				this.status.textContent = nls.localize('devices.idCopied', 'Device ID copied.');
			}
		} catch {
			if (!this.isDisposed() && this.device?.id === id) {
				this.status.textContent = nls.localize('devices.copyFailed', 'Unable to copy the device ID.');
			}
		}
	}

	private setRenaming(renaming: boolean): void {
		const wasRenaming = !this.renameForm.hidden;
		this.renameForm.hidden = !renaming;
		this.title.hidden = renaming;
		this.renameButton.hidden = renaming;
		if (renaming) {
			this.name.value = this.device?.name ?? '';
			this.name.focus();
			this.name.select();
		} else if (wasRenaming && this.renameForm.contains(document.activeElement)) {
			this.renameButton.focus();
		}
		this.updateControls();
	}

	private submitName(): void {
		if (this.save.enabled) {
			const name = this.name.value.trim();
			this.run(target => this.devices.rename(target, name), () => {
				this.setRenaming(false);
			});
		}
	}

	private updateControls(): void {
		const device = this.device;
		const busy = !!device && this.pending.has(device.id);
		const enabled = !!device && this.devices.available && !busy;
		const name = this.name.value.trim();
		if (enabled) {
			this.name.enable();
		} else {
			this.name.disable();
		}
		this.save.enabled = enabled && name.length > 0 && name !== device.name;
		this.renameButton.disabled = !enabled;
		this.forget.disabled = !enabled;
		this.keep.enabled = enabled;
		this.confirmForget.enabled = enabled;
		this.element.setAttribute('aria-busy', String(busy));
	}

	/** Runs a command for the selected computer; its outcome is shown only while it stays selected. */
	private run(command: (id: string) => Promise<void>, onSuccess?: () => void): void {
		const id = this.device?.id;
		if (!id || this.pending.has(id)) {
			return;
		}
		this.pending.add(id);
		this.status.textContent = '';
		this.updateControls();
		command(id).then(() => {
			if (!this.isDisposed() && this.device?.id === id) {
				onSuccess?.();
			}
		}, (error: unknown) => {
			if (!this.isDisposed() && this.device?.id === id) {
				this.status.textContent = error instanceof Error ? error.message : nls.localize('devices.actionFailed', 'The device action failed. Try again.');
			}
		}).finally(() => {
			this.pending.delete(id);
			if (!this.isDisposed()) {
				this.updateControls();
			}
		});
	}

	/** Re-read after every await: the page can close while a command is in flight. */
	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}

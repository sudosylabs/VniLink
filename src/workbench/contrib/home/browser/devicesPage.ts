import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { Icons } from '../../../../base/common/icons.js';
import { renderIcon } from '../../../../base/browser/ui/icon/icon.js';
import { IDeviceService, type DeviceSummary } from '../../../services/devices/common/deviceService.js';
import type { TabAction } from '../../../services/tabs/common/tabService.js';
import { createPageHeading, type WorkbenchPage } from '../../../browser/parts/views/page.js';
import './media/devices.css';

export class DevicesPage extends Disposable implements WorkbenchPage {
	public readonly element = document.createElement('main');
	public readonly actions: readonly TabAction[];
	private readonly list = document.createElement('section');

	constructor(@IDeviceService devices: IDeviceService) {
		super();
		this.list.className = 'empty-state';
		this.list.setAttribute('aria-label', nls.localize('computers.sectionLabel', 'Computers'));
		this.element.append(createPageHeading(
			nls.localize('computers.heading', 'Devices'),
			nls.localize('computers.subheading', 'The computers you connect to.'),
		), this.list);
		this.actions = [{
			id: 'refresh',
			label: nls.localize('computers.refreshLabel', 'Refresh devices'),
			icon: Icons.refresh,
			enabled: devices.available,
			run: signal => devices.refresh(signal),
		}];
		this._register(devices.onDidChangeDevices(devices => {
			this.render(devices);
		}));
		this.render(devices.devices);
		void devices.initialize();
	}

	private render(devices: readonly DeviceSummary[]): void {
		this.list.replaceChildren();
		this.list.className = devices.length ? 'device-list' : 'empty-state';
		if (devices.length) {
			this.list.setAttribute('role', 'list');
		} else {
			this.list.removeAttribute('role');
		}
		if (!devices.length) {
			const icon = renderIcon(Icons.computers);
			icon.classList.add('computer');
			const title = document.createElement('h2');
			title.textContent = nls.localize('computers.emptyTitle', 'No devices yet');
			const description = document.createElement('p');
			description.textContent = nls.localize('computers.emptyDescription', 'Paired devices will appear here.');
			this.list.append(icon, title, description);
			return;
		}
		for (const device of devices) {
			const row = document.createElement('div');
			row.className = 'device-row';
			row.setAttribute('role', 'listitem');
			const label = document.createElement('span');
			label.textContent = device.name;
			const details = document.createElement('span');
			details.className = 'device-details';
			const platform = device.os === 'macos' ? 'macOS' : device.os === 'windows' ? 'Windows' : device.os === 'linux' ? 'Linux' : nls.localize('devices.computer', 'Computer');
			const states = {
				connected: nls.localize('devices.connected', 'Connected'),
				connecting: nls.localize('devices.connecting', 'Connecting'),
				reconnecting: nls.localize('devices.reconnecting', 'Reconnecting'),
				disconnected: nls.localize('devices.disconnected', 'Disconnected'),
				unknown: nls.localize('devices.unknown', 'Connection status unknown'),
			};
			details.textContent = `${platform} · ${states[device.presence]}`;
			row.append(renderIcon(Icons.remoteDesktop), label, details);
			this.list.append(row);
		}
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}

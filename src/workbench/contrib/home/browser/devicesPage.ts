import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { Icons } from '../../../../base/common/icons.js';
import { renderIcon } from '../../../../base/browser/ui/icon/icon.js';
import { IDeviceService, type DeviceSummary } from '../../../services/devices/common/deviceService.js';
import type { TabAction } from '../../../services/tabs/common/tabService.js';
import { createPageHeading, type WorkbenchPage } from '../../../browser/parts/views/page.js';

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
	}

	private render(devices: readonly DeviceSummary[]): void {
		this.list.replaceChildren();
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
			const label = document.createElement('p');
			label.textContent = device.name;
			this.list.append(label);
		}
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}

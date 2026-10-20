import { localize2 } from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { Action2 } from '../../../platform/actions/common/actions.js';
import { IInstantiationService, type ServicesAccessor } from '../../../platform/instantiation/common/instantiation.js';
import { IDeviceService } from '../../services/devices/deviceService.js';
import { ITabService } from '../../services/tabs/tabService.js';
import { RemoteDesktopView } from './remoteDesktopView.js';

/** Shows the screen of the paired computer given by id in its one tab, controlling it or only viewing it. */
function openDesktop(accessor: ServicesAccessor, deviceId: unknown, control: boolean): void {
	const device = typeof deviceId === 'string' ? accessor.get(IDeviceService).devices.find(candidate => candidate.id === deviceId) : undefined;
	if (!device) {
		return;
	}
	const tabs = accessor.get(ITabService);
	const id = RemoteDesktopView.idFor(device.id);
	if (tabs.tabs.some(tab => tab.id === id)) {
		tabs.select(id);
	} else {
		tabs.open(accessor.get(IInstantiationService).createInstance(RemoteDesktopView, device, control));
	}
}

export class OpenRemoteDesktopAction extends Action2 {
	public static readonly ID = 'vnilink.desktop.open';

	constructor() {
		super({
			id: OpenRemoteDesktopAction.ID,
			title: localize2('desktop.open', 'Open Remote Desktop'),
			icon: ThemeIcon.fromId(Icons.remoteDesktop),
		});
	}

	public override run(accessor: ServicesAccessor, deviceId?: unknown): void {
		openDesktop(accessor, deviceId, true);
	}
}

export class ViewScreenAction extends Action2 {
	public static readonly ID = 'vnilink.desktop.view';

	constructor() {
		super({
			id: ViewScreenAction.ID,
			title: localize2('desktop.view', 'View Screen'),
			icon: ThemeIcon.fromId(Icons.view),
		});
	}

	public override run(accessor: ServicesAccessor, deviceId?: unknown): void {
		openDesktop(accessor, deviceId, false);
	}
}

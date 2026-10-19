import { localize2 } from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { Action2 } from '../../../platform/actions/common/actions.js';
import { IInstantiationService, type ServicesAccessor } from '../../../platform/instantiation/common/instantiation.js';
import { IDeviceService } from '../../services/devices/deviceService.js';
import { ITabService } from '../../services/tabs/tabService.js';
import { FilesView } from './filesView.js';

/** Shows this computer's and the paired computer's files, given by id, in its one Files tab. */
export class OpenFilesAction extends Action2 {
	public static readonly ID = 'vnilink.files.open';

	constructor() {
		super({
			id: OpenFilesAction.ID,
			title: localize2('files.open', 'Open Files'),
			icon: ThemeIcon.fromId(Icons.files),
		});
	}

	public override run(accessor: ServicesAccessor, deviceId?: unknown): void {
		const device = typeof deviceId === 'string' ? accessor.get(IDeviceService).devices.find(candidate => candidate.id === deviceId) : undefined;
		if (!device) {
			return;
		}
		const tabs = accessor.get(ITabService);
		const id = FilesView.idFor(device.id);
		if (tabs.tabs.some(tab => tab.id === id)) {
			tabs.select(id);
		} else {
			tabs.open(accessor.get(IInstantiationService).createInstance(FilesView, device));
		}
	}
}

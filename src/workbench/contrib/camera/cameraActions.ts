import { localize2 } from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { Action2 } from '../../../platform/actions/common/actions.js';
import { IInstantiationService, type ServicesAccessor } from '../../../platform/instantiation/common/instantiation.js';
import { IDeviceService } from '../../services/devices/deviceService.js';
import { ITabService } from '../../services/tabs/tabService.js';
import { CameraView } from './cameraView.js';

/** Shows the camera of the paired computer given by id, in its one camera tab. */
export class OpenCameraAction extends Action2 {
	public static readonly ID = 'vnilink.camera.open';

	constructor() {
		super({
			id: OpenCameraAction.ID,
			title: localize2('camera.open', 'Open Camera'),
			icon: ThemeIcon.fromId(Icons.camera),
		});
	}

	public override run(accessor: ServicesAccessor, deviceId?: unknown): void {
		const device = typeof deviceId === 'string' ? accessor.get(IDeviceService).devices.find(candidate => candidate.id === deviceId) : undefined;
		if (!device) {
			return;
		}
		const tabs = accessor.get(ITabService);
		const id = CameraView.idFor(device.id);
		if (tabs.tabs.some(tab => tab.id === id)) {
			tabs.select(id);
		} else {
			tabs.open(accessor.get(IInstantiationService).createInstance(CameraView, device));
		}
	}
}

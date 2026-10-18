import { localize2 } from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { Action2 } from '../../../platform/actions/common/actions.js';
import { IInstantiationService, type ServicesAccessor } from '../../../platform/instantiation/common/instantiation.js';
import { IDeviceService } from '../../services/devices/deviceService.js';
import { ITabService } from '../../services/tabs/tabService.js';
import { TerminalView } from './terminalView.js';

/** Opens a new terminal tab with a shell on the paired computer given by id. */
export class OpenTerminalAction extends Action2 {
	public static readonly ID = 'vnilink.terminal.open';

	constructor() {
		super({
			id: OpenTerminalAction.ID,
			title: localize2('terminal.open', 'Open Terminal'),
			icon: ThemeIcon.fromId(Icons.terminal),
		});
	}

	public override run(accessor: ServicesAccessor, deviceId?: unknown): void {
		const device = typeof deviceId === 'string' ? accessor.get(IDeviceService).devices.find(candidate => candidate.id === deviceId) : undefined;
		if (device) {
			accessor.get(ITabService).open(accessor.get(IInstantiationService).createInstance(TerminalView, device));
		}
	}
}

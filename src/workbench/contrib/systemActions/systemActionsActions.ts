import * as nls from '../../../nls.js';
import { localize2 } from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { Action2 } from '../../../platform/actions/common/actions.js';
import type { ServicesAccessor } from '../../../platform/instantiation/common/instantiation.js';
import { IDeviceService, type DeviceSummary } from '../../services/devices/deviceService.js';
import { IDialogService } from '../../services/dialogs/dialogService.js';
import { ISystemActionService } from '../../services/systemActions/systemActionService.js';

/** Locks the paired computer given by id. Signing back in undoes it, so it does not ask first. */
export class LockComputerAction extends Action2 {
	public static readonly ID = 'vnilink.system.lock';

	constructor() {
		super({
			id: LockComputerAction.ID,
			title: localize2('system.lock', 'Lock Screen'),
			icon: ThemeIcon.fromId(Icons.lock),
		});
	}

	public override async run(accessor: ServicesAccessor, deviceId?: unknown): Promise<void> {
		if (typeof deviceId === 'string') {
			await accessor.get(ISystemActionService).run(deviceId, 'lock');
		}
	}
}

/**
 * Asks whether to restart or shut down the paired computer given by id,
 * offering only what that computer allows, then asks it to.
 */
export class RestartOrShutDownAction extends Action2 {
	public static readonly ID = 'vnilink.system.restartOrShutDown';
	/** Computers with a confirmation open: pressing the row again shows no second one. */
	private static readonly confirming = new Set<string>();

	constructor() {
		super({
			id: RestartOrShutDownAction.ID,
			title: localize2('system.restartOrShutDown', 'Restart or Shut Down…'),
			icon: ThemeIcon.fromId(Icons.power),
		});
	}

	public override async run(accessor: ServicesAccessor, deviceId?: unknown): Promise<void> {
		const devices = accessor.get(IDeviceService);
		const dialogs = accessor.get(IDialogService);
		const systemActions = accessor.get(ISystemActionService);
		const device = typeof deviceId === 'string' ? devices.devices.find(candidate => candidate.id === deviceId) : undefined;
		if (!device || RestartOrShutDownAction.confirming.has(device.id)) {
			return;
		}
		RestartOrShutDownAction.confirming.add(device.id);
		try {
			await RestartOrShutDownAction.confirm(device, devices, dialogs, systemActions);
		} finally {
			RestartOrShutDownAction.confirming.delete(device.id);
		}
	}

	private static async confirm(device: DeviceSummary, devices: IDeviceService, dialogs: IDialogService, systemActions: ISystemActionService): Promise<void> {
		const report = await devices.availability(device.id).catch(() => null);
		const allowed = (capability: 'systemRestart' | 'systemShutdown'): boolean => report?.find(entry => entry.capability === capability)?.status === 'available';
		const choices = [
			...allowed('systemRestart') ? [{ label: nls.localize('system.restartButton', 'Restart'), action: 'restart' as const }] : [],
			...allowed('systemShutdown') ? [{ label: nls.localize('system.shutDownButton', 'Shut Down'), action: 'shutdown' as const }] : [],
		];
		if (!choices.length) {
			return;
		}
		const message = choices.length === 2
			? nls.localize('system.confirmBoth', 'Restart or shut down {0}?', device.name)
			: choices[0].action === 'restart'
				? nls.localize('system.confirmRestart', 'Restart {0}?', device.name)
				: nls.localize('system.confirmShutDown', 'Shut down {0}?', device.name);
		const { button } = await dialogs.show({
			type: 'warning',
			message,
			detail: nls.localize('system.confirmDetail', 'Its user has 30 seconds to cancel. Anything not saved there may be lost.'),
			buttons: [...choices.map(choice => choice.label), nls.localize('system.cancelButton', 'Cancel')],
			cancelId: choices.length,
		});
		const chosen = choices.at(button);
		if (chosen && button < choices.length) {
			await systemActions.run(device.id, chosen.action);
		}
	}
}

/** Cancels the restart or shutdown counting down on the paired computer given by id. */
export class CancelSystemActionAction extends Action2 {
	public static readonly ID = 'vnilink.system.cancel';

	constructor() {
		super({
			id: CancelSystemActionAction.ID,
			title: localize2('system.cancelAction', 'Cancel Restart or Shutdown'),
		});
	}

	public override async run(accessor: ServicesAccessor, deviceId?: unknown): Promise<void> {
		if (typeof deviceId === 'string') {
			await accessor.get(ISystemActionService).cancel(deviceId);
		}
	}
}

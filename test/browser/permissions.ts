import { Emitter } from '../../src/base/common/event.js';
import { MachinePermissionsSection } from '../../src/workbench/contrib/settings/browser/machinePermissionsSection.js';
import type { IPermissionService } from '../../src/workbench/services/permissions/common/permissionService.js';
import { permissionKinds, type PermissionCommand, type PermissionSnapshot } from '../../src/platform/permissions/common/permissions.js';

function assert(condition: boolean, message: string): void {
	if (!condition) {
		throw new Error(message);
	}
}

async function settle(): Promise<void> {
	await new Promise<void>(resolve => {
		setTimeout(resolve, 0);
	});
}

export async function verifyMachinePermissions(): Promise<void> {
	const focused = new Emitter<void>();
	const commands: PermissionCommand[] = [];
	let fail = false;
	let cameraAllowed = false;
	const snapshot = (): PermissionSnapshot => ({ development: true, permissions: permissionKinds.map(kind => ({ kind, status: kind === 'camera' ? (cameraAllowed ? 'granted' : 'not-determined') : kind === 'screen' ? 'denied' : 'unsupported', canRequest: kind === 'camera' && !cameraAllowed, canOpenSettings: kind === 'camera' || kind === 'screen' })) });
	const service: IPermissionService = {
		_serviceBrand: undefined, onDidFocusWindow: focused.event, execute: command => {
			commands.push(command);
			if (fail) {
				return Promise.reject(new Error('OS query failed'));
			}
			if (command.type === 'request') {
				cameraAllowed = true;
			}
			return Promise.resolve(snapshot());
		}
	};
	const section = new MachinePermissionsSection(service);
	try {
		await settle();
		assert(commands.length === 1 && commands[0].type === 'status', 'Loading page prompted for permissions');
		const camera = section.element.querySelector<HTMLButtonElement>('[data-permission-kind="camera"] button');
		const screen = section.element.querySelector<HTMLButtonElement>('[data-permission-kind="screen"] button');
		assert(!!camera && camera.textContent === 'Request access', 'Undetermined camera did not offer request');
		assert(!!screen && screen.textContent === 'Open settings', 'Screen permission did not offer settings');
		assert(section.element.querySelector<HTMLInputElement>('input') === null, 'OS permissions rendered as preference switches');
		camera?.click();
		await settle();
		assert(commands.some(c => c.type === 'request' && c.kind === 'camera'), 'Explicit camera action did not request access');
		assert(section.element.querySelector('[data-permission-kind="camera"] [data-status="granted"]') !== null, 'Actual granted status not rendered');
		screen?.click();
		await settle();
		assert(commands.some(c => c.type === 'openSettings' && c.kind === 'screen'), 'Screen action did not open settings');
		cameraAllowed = false;
		focused.fire();
		await settle();
		assert(camera?.textContent === 'Request access', 'Window focus did not refresh OS status');
		fail = true;
		focused.fire();
		await settle();
		assert(camera?.disabled === true && section.element.querySelector('[data-status="granted"]') === null, 'Query failure kept stale granted state');
		section.dispose();
		const count = commands.length;
		focused.fire();
		camera?.click();
		await settle();
		assert(commands.length === count, 'Disposed permission page retained live actions');
	} finally {
		section.dispose();
		focused.dispose();
	}
}

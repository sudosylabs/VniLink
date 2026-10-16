import { Emitter } from '../../src/base/common/event.js';
import { SharingSection } from '../../src/workbench/contrib/settings/browser/sharingSection.js';
import { LocalSecuritySection } from '../../src/workbench/contrib/settings/browser/localSecuritySection.js';
import type { IAccessService, LocalSecurity } from '../../src/workbench/services/access/common/accessService.js';
import type { SharingPolicy } from '../../src/platform/engine/common/engine.js';

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

export async function verifyAccess(): Promise<void> {
	let sharing: SharingPolicy = { view: false, control: false, clipboard: false, files: false, terminal: false, camera: false, system_audio: false, microphone: false, recording: false, system_actions: false };
	let fail = false;
	let writes = 0;
	const security: LocalSecurity = { policy: { allow_lock: true, allow_restart: false, allow_shutdown: false, allow_privacy: false }, support: { lock: true, restart: false, shutdown: false, privacy: false } };
	const service: IAccessService = {
		_serviceBrand: undefined, available: true,
		loadSharing: () => Promise.resolve(sharing),
		setSharing: changes => {
			writes++;
			if (fail) {
				return Promise.reject(new Error('write failed'));
			}
			sharing = { ...sharing, ...changes };
			return Promise.resolve(sharing);
		},
		loadPolicy: () => Promise.resolve(security),
		setPolicy: () => Promise.reject(new Error('policy write failed')),
	};
	const focused = new Emitter<void>();
	const permissions = { _serviceBrand: undefined, onDidFocusWindow: focused.event, execute: () => Promise.reject(new Error('Not exercised')) };
	const section = new SharingSection(service, permissions);
	const local = new LocalSecuritySection(service, permissions);
	document.body.append(section.element, local.element);
	try {
		await settle();
		const screen = section.element.querySelector<HTMLInputElement>('[data-sharing="view"]');
		assert(!!screen && !screen.disabled && !screen.checked, 'Global sharing did not load on an installation without paired devices');
		if (!screen) {
			return;
		}
		screen.checked = true;
		screen.dispatchEvent(new Event('change'));
		await settle();
		assert(sharing.view && sharing.control && !sharing.files, 'Screen setting did not enable view/control together or changed another service');
		fail = true;
		screen.checked = false;
		screen.dispatchEvent(new Event('change'));
		await settle();
		assert(screen.matches(':checked') && !screen.disabled, 'Failed global write did not restore confirmed state');
		const lock = local.element.querySelector<HTMLInputElement>('[data-policy="allow_lock"]');
		assert(!!lock && lock.checked, 'System action rules did not load');
		assert(local.element.querySelector<HTMLInputElement>('[data-policy="allow_restart"]')?.disabled === true, 'Unsupported system action was editable');
		section.dispose();
		const count = writes;
		screen.dispatchEvent(new Event('change'));
		await settle();
		assert(writes === count, 'Disposed sharing controls still wrote policy');
	} finally {
		section.dispose();
		local.dispose();
		section.element.remove();
		local.element.remove();
		focused.dispose();
	}
}

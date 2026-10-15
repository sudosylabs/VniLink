import { Emitter } from '../../src/base/common/event.js';
import { PeerAccessSection } from '../../src/workbench/contrib/settings/browser/peerAccessSection.js';
import { LocalSecuritySection } from '../../src/workbench/contrib/settings/browser/localSecuritySection.js';
import type { IAccessService, PeerAccess, AccessPermission, LocalSecurity } from '../../src/workbench/services/access/common/accessService.js';

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
	const permissions: Record<AccessPermission, boolean> = { view: false, control: false, clipboard: false, files: false, terminal: false, camera: false, systemAudio: false, microphone: false, recording: false, systemActions: false };
	const peer: PeerAccess = { device: { id: 'peer-one', name: '<Work>' }, permissions };
	let fail = false;
	let requestedPeer = '';
	const security: LocalSecurity = { policy: { allow_lock: true, allow_restart: false, allow_shutdown: false, allow_privacy: false }, support: { lock: true, restart: false, shutdown: false, privacy: false } };
	const service: IAccessService = {
		_serviceBrand: undefined, available: true,
		listDevices: () => Promise.resolve([peer.device]), loadPeer: () => Promise.resolve(peer), loadPolicy: () => Promise.resolve(security),
		setPermission: (id, key, allowed) => {
			requestedPeer = id;
			if (fail) {
				return Promise.reject(new Error('write failed'));
			}
			permissions[key] = allowed;
			return Promise.resolve(peer);
		},
		setPolicy: () => Promise.reject(new Error('policy write failed')),
	};
	const focused = new Emitter<void>();
	const section = new PeerAccessSection(service);
	const local = new LocalSecuritySection(service, { _serviceBrand: undefined, onDidFocusWindow: focused.event, execute: () => Promise.reject(new Error('Not exercised')) });
	document.body.append(local.element);
	try {
		await settle();
		const control = section.element.querySelector<HTMLInputElement>('[data-permission="control"]');
		assert(!!control && !control.disabled, 'Peer permissions failed to load');
		if (!control) {
			return;
		}
		control.checked = true;
		control.dispatchEvent(new Event('change'));
		await settle();
		assert(requestedPeer === 'peer-one' && permissions.control, 'Permission change targeted wrong identity');
		fail = true;
		control.checked = false;
		control.dispatchEvent(new Event('change'));
		await settle();
		assert(section.element.querySelector<HTMLInputElement>('[data-permission="control"]')?.checked === true, 'Failed change did not restore last known permission');
		assert(control.disabled && section.element.querySelector<HTMLElement>('.access-permissions')?.hidden === true, 'Unconfirmed permission remained editable');
		const lock = local.element.querySelector<HTMLInputElement>('[data-policy="allow_lock"]');
		assert(!!lock && lock.checked, 'Local policy was not loaded independently');
		let completeRead: ((state: LocalSecurity) => void) | undefined;
		service.loadPolicy = () => new Promise<LocalSecurity>(resolve => {
			completeRead = resolve;
		});
		focused.fire();
		assert(lock?.disabled === false, 'Background status read disabled a confirmed policy control');
		completeRead?.(security);
		await settle();
		assert(local.element.querySelector<HTMLInputElement>('[data-policy="allow_restart"]')?.disabled === true, 'Unsupported system action remained editable');
		assert(local.element.textContent.includes('Not supported on this computer'), 'Backend support was misrepresented');
		if (lock) {
			lock.checked = false;
			lock.dispatchEvent(new Event('change'));
			await settle();
			assert(local.element.querySelector<HTMLInputElement>('[data-policy="allow_lock"]')?.checked === true, 'Failed policy change did not restore last known state');
			assert(lock.disabled, 'Unconfirmed policy remained editable');
		}
		section.dispose();
		requestedPeer = '';
		control.dispatchEvent(new Event('change'));
		await settle();
		assert(requestedPeer === '', 'Disposed controls still write permissions');
	} finally {
		section.dispose();
		local.dispose();
		local.element.remove();
		focused.dispose();
	}
	const empty = new PeerAccessSection({ ...service, listDevices: () => Promise.resolve([]) });
	try {
		await settle();
		assert(empty.element.textContent.includes('No paired devices'), 'Empty installation did not explain paired-device requirement');
		assert(empty.element.querySelector<HTMLElement>('.access-permissions')?.hidden === true, 'Empty installation displayed fake grants');
	} finally {
		empty.dispose();
	}
}

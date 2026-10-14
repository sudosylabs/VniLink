import type { DesktopApi } from '../../../../platform/ipc/common/desktopApi.js';
import type { Device, ServiceKind, EngineReply, SystemPolicy, SystemSupport } from '../../../../platform/engine/common/engine.js';
import { AccessReadbackError, type AccessPermission, type PeerAccess, type LocalSecurity, type IAccessService } from '../common/accessService.js';
import type { DeviceSummary } from '../../devices/common/deviceService.js';
const systemCapabilities: Record<keyof SystemPolicy, keyof SystemSupport> = { allow_lock: 'lock', allow_restart: 'restart', allow_shutdown: 'shutdown', allow_privacy: 'privacy' };
const services: Record<Exclude<AccessPermission, 'control' | 'clipboard' | 'recording'>, ServiceKind> = {
	view: 'Desktop', files: 'Files', terminal: 'Terminal', camera: 'Camera', systemAudio: 'SystemAudio', microphone: 'Microphone', systemActions: 'System',
};

/** Each write changes one permission, with fresh reads preserving unrelated grants. */
export class AccessService implements IAccessService {
	declare public readonly _serviceBrand: undefined;
	private pending: Promise<unknown> = Promise.resolve();
	constructor(private readonly api: Pick<DesktopApi, 'engineCommand'>, public readonly available: boolean) { }
	private async devices(): Promise<readonly Device[]> {
		if (!this.available) {
			throw new Error('Access service unavailable');
		}
		const reply = await this.api.engineCommand({ type: 'snapshot' });
		if (reply.type !== 'snapshot') {
			throw new Error('Invalid device reply');
		}
		return reply.snapshot.devices;
	}
	public async listDevices(): Promise<readonly DeviceSummary[]> {
		return (await this.devices()).map(device => ({ id: device.endpoint_id, name: device.pet_name ?? device.hostname ?? device.endpoint_id }));
	}
	private async device(id: string): Promise<Device> {
		const device = (await this.devices()).find(device => device.endpoint_id === id);
		if (!device) {
			throw new Error('Device is no longer paired');
		}
		return device;
	}
	public async loadPeer(id: string): Promise<PeerAccess> {
		const device = await this.device(id);
		const reply = await this.api.engineCommand({ type: 'servicePermissions', endpointId: id });
		if (reply.type !== 'servicePermissions') {
			throw new Error('Invalid permissions reply');
		}
		const permissions = reply.permissions;
		return {
			device: { id, name: device.pet_name ?? device.hostname ?? id },
			permissions: { view: permissions.desktop, control: device.grants.control, clipboard: device.grants.clipboard, files: permissions.files, terminal: permissions.terminal, camera: permissions.camera, systemAudio: permissions.system_audio, microphone: permissions.microphone, recording: permissions.recording, systemActions: permissions.system_actions },
		};
	}
	public setPermission(id: string, permission: AccessPermission, allowed: boolean): Promise<PeerAccess> {
		return this.queue(async () => {
			const device = await this.device(id);
			let reply: EngineReply;
			if (permission === 'control' || permission === 'clipboard') {
				reply = await this.api.engineCommand({ type: 'authorize', endpointId: id, ...device.grants, [permission]: allowed });
			} else if (permission === 'recording') {
				reply = await this.api.engineCommand({ type: 'authorizeRecording', endpointId: id, allowed });
			} else {
				reply = await this.api.engineCommand({ type: 'authorizeService', endpointId: id, kind: services[permission], allowed });
			}
			if (reply.type !== 'done') {
				throw new Error('Invalid permission write reply');
			}
			try {
				return await this.loadPeer(id);
			} catch {
				throw new AccessReadbackError();
			}
		});
	}
	public async loadPolicy(): Promise<LocalSecurity> {
		if (!this.available) {
			throw new Error('Access service unavailable');
		}
		const reply = await this.api.engineCommand({ type: 'systemPolicy' });
		if (reply.type !== 'systemPolicy') {
			throw new Error('Invalid security policy reply');
		}
		return { policy: reply.policy, support: reply.support };
	}
	public setPolicy(key: keyof SystemPolicy, allowed: boolean): Promise<LocalSecurity> {
		return this.queue(async () => {
			const current = await this.loadPolicy();
			if (!current.support[systemCapabilities[key]]) {
				throw new Error('System action unavailable');
			}
			const reply = await this.api.engineCommand({ type: 'setSystemPolicy', policy: { ...current.policy, [key]: allowed } });
			if (reply.type !== 'done') {
				throw new Error('Invalid policy write reply');
			}
			try {
				return await this.loadPolicy();
			} catch {
				throw new AccessReadbackError();
			}
		});
	}
	private queue<T>(task: () => Promise<T>): Promise<T> {
		const result = this.pending.then(task); this.pending = result.catch(() => undefined); return result;
	}
}

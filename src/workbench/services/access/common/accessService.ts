import { createDecorator } from '../../../../platform/instantiation/common/instantiation.js';
import type { SystemPolicy, SystemSupport } from '../../../../platform/engine/common/engine.js';
import type { DeviceSummary } from '../../devices/common/deviceService.js';
export type AccessPermission = 'view' | 'control' | 'clipboard' | 'files' | 'terminal' | 'camera' | 'systemAudio' | 'microphone' | 'recording' | 'systemActions';
export interface PeerAccess { readonly device: DeviceSummary; readonly permissions: Readonly<Record<AccessPermission, boolean>>; }
export interface LocalSecurity { readonly policy: Readonly<SystemPolicy>; readonly support: Readonly<SystemSupport>; }
export const IAccessService = createDecorator<IAccessService>('accessService');
export interface IAccessService {
	readonly _serviceBrand: undefined;
	readonly available: boolean;
	listDevices(): Promise<readonly DeviceSummary[]>;
	loadPeer(id: string): Promise<PeerAccess>;
	setPermission(id: string, permission: AccessPermission, allowed: boolean): Promise<PeerAccess>;
	loadPolicy(): Promise<LocalSecurity>;
	setPolicy(key: keyof SystemPolicy, allowed: boolean): Promise<LocalSecurity>;
}
export class AccessReadbackError extends Error {
	constructor() {
		super('Access change committed, but read-back failed');
	}
}

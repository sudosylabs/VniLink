import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { SharingPolicy, SystemPolicy, SystemSupport } from '../../../platform/engine/common/engine.js';

export interface LocalSecurity { readonly policy: Readonly<SystemPolicy>; readonly support: Readonly<SystemSupport>; }

export const IAccessService = createDecorator<IAccessService>('accessService');

export interface IAccessService {
	readonly _serviceBrand: undefined;
	readonly available: boolean;
	loadSharing(): Promise<Readonly<SharingPolicy>>;
	setSharing(changes: Partial<SharingPolicy>): Promise<Readonly<SharingPolicy>>;
	loadPolicy(): Promise<LocalSecurity>;
	setPolicy(key: keyof SystemPolicy, allowed: boolean): Promise<LocalSecurity>;
}

export class AccessReadbackError extends Error {
	constructor() {
		super('Access change committed, but read-back failed');
	}
}

/** A write can fail during native cleanup after consent was already persisted. */
export class AccessWriteError extends Error {
	constructor(public readonly policy: Readonly<SharingPolicy> | undefined, cause: unknown) {
		super('Sharing update failed; reconcile with the observed policy', { cause });
	}
}

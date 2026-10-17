import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import type { SharingPolicy, SystemPolicy, SystemSupport } from '../../../platform/engine/common/engine.js';
import { AccessReadbackError, AccessWriteError, type LocalSecurity, IAccessService } from './accessService.js';

const systemCapabilities: Record<keyof SystemPolicy, keyof SystemSupport> = { allow_lock: 'lock', allow_restart: 'restart', allow_shutdown: 'shutdown', allow_privacy: 'privacy' };

/** Serialized writes patch global consent atomically in the engine. */
export class AccessService implements IAccessService {
	declare public readonly _serviceBrand: undefined;
	private pending: Promise<unknown> = Promise.resolve();

	constructor(@IEngineService private readonly engine: IEngineService) { }

	public get available(): boolean {
		return this.engine.available;
	}

	public async loadSharing(): Promise<Readonly<SharingPolicy>> {
		if (!this.available) {
			throw new Error('Sharing policy unavailable');
		}
		const reply = await this.engine.execute({ type: 'sharingPolicy' });
		if (reply.type !== 'sharingPolicy') {
			throw new Error('Invalid sharing policy reply');
		}
		return Object.freeze({ ...reply.policy });
	}

	public setSharing(changes: Partial<SharingPolicy>): Promise<Readonly<SharingPolicy>> {
		return this.queue(async () => {
			if (!this.available) {
				throw new Error('Sharing policy unavailable');
			}
			try {
				const reply = await this.engine.execute({ type: 'setSharingPolicy', policy: changes });
				if (reply.type !== 'done') {
					throw new Error('Invalid sharing policy update reply');
				}
			} catch (error) {
				let policy: Readonly<SharingPolicy> | undefined;
				try {
					policy = await this.loadSharing();
				} catch { /* Keep state unknown until a successful read. */ }
				throw new AccessWriteError(policy, error);
			}
			try {
				return await this.loadSharing();
			} catch {
				throw new AccessReadbackError();
			}
		});
	}

	public async loadPolicy(): Promise<LocalSecurity> {
		if (!this.available) {
			throw new Error('Access service unavailable');
		}
		const reply = await this.engine.execute({ type: 'systemPolicy' });
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
			const reply = await this.engine.execute({ type: 'setSystemPolicy', policy: { ...current.policy, [key]: allowed } });
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
		const result = this.pending.then(task);
		this.pending = result.catch(() => undefined);
		return result;
	}
}

registerSingleton(IAccessService, AccessService, InstantiationType.Delayed);

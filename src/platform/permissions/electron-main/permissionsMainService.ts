import { app, shell, systemPreferences } from 'electron';
import { platform } from '../../../base/common/process.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { IEngineMainService } from '../../engine/electron-main/engineMainService.js';
import type { EngineMainService } from '../../engine/electron-main/engineMainService.js';
import { synchronizeNativePermissions } from '../../engine/electron-main/nativeRuntime.js';
import type { PermissionCommand, PermissionSnapshot } from '../common/permissions.js';
import { MachinePermissions } from './machinePermissions.js';

export const IPermissionsMainService = createDecorator<PermissionsMainService>('permissionsMainService');

/**
 * Reads live OS consent and tells the engine which native backends it may use.
 * Synchronization repeats only when the reported consent changes.
 */
export class PermissionsMainService {
	declare public readonly _serviceBrand: undefined;
	private readonly machine = new MachinePermissions(platform, !app.isPackaged, systemPreferences, url => shell.openExternal(url));
	private synchronized: string | undefined;

	constructor(@IEngineMainService private readonly engine: EngineMainService) { }

	public snapshot(): PermissionSnapshot {
		return this.machine.snapshot();
	}

	public isGranted(kind: PermissionSnapshot['permissions'][number]['kind']): boolean {
		return this.snapshot().permissions.some(permission => permission.kind === kind && permission.status === 'granted');
	}

	public async execute(command: PermissionCommand): Promise<PermissionSnapshot> {
		const snapshot = await this.machine.execute(command);
		const key = JSON.stringify(snapshot.permissions.map(permission => [permission.kind, permission.status]));
		if (this.engine.available && key !== this.synchronized) {
			await synchronizeNativePermissions(this.engine, platform, snapshot).then(() => {
				this.synchronized = key;
			}, () => { /* Keep OS state truthful; retry failed backend synchronization on the next read. */ });
		}
		return snapshot;
	}

	/** Reports current consent to the engine regardless of earlier synchronization. */
	public synchronize(): Promise<void> {
		return synchronizeNativePermissions(this.engine, platform, this.snapshot());
	}
}

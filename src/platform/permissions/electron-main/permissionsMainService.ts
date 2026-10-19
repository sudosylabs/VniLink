import { desktopCapturer, shell, systemPreferences } from 'electron';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { platform } from '../../../base/common/process.js';
import { isBuilt } from '../../environment/electron-main/environment.js';
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
export class PermissionsMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly consentChanged = this._register(new Emitter<void>());
	/** Fires when reported OS consent changes, such as Screen Recording being granted. */
	public readonly onDidChangeConsent = this.consentChanged.event;
	private readonly machine = new MachinePermissions(platform, !isBuilt(), systemPreferences, url => shell.openExternal(url), async () => {
		// Asking for capture sources is what makes macOS prompt and list the app.
		await desktopCapturer.getSources({ types: ['screen'], thumbnailSize: { width: 0, height: 0 } });
	});
	private synchronized: string | undefined;

	constructor(@IEngineMainService private readonly engine: EngineMainService) {
		super();
	}

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
			const changed = this.synchronized !== undefined;
			await synchronizeNativePermissions(this.engine, platform, snapshot).then(() => {
				this.synchronized = key;
				if (changed) {
					this.consentChanged.fire();
				}
			}, () => { /* Keep OS state truthful; retry failed backend synchronization on the next read. */ });
		}
		return snapshot;
	}

	/** Reports current consent to the engine regardless of earlier synchronization. */
	public async synchronize(): Promise<void> {
		const snapshot = this.snapshot();
		await synchronizeNativePermissions(this.engine, platform, snapshot);
		this.synchronized = JSON.stringify(snapshot.permissions.map(permission => [permission.kind, permission.status]));
	}
}

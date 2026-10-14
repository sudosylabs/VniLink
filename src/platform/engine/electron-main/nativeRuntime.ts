import type { PermissionSnapshot } from '../../permissions/common/permissions.js';
import type { EngineService } from './engineService.js';
/** No permission prompts, capture sessions or shell processes are started here. */
export async function synchronizeNativePermissions(engine: Pick<EngineService, 'available' | 'execute'>, platform: string, snapshot: PermissionSnapshot): Promise<void> {
	if (!engine.available) {
		return;
	}
	if (platform === 'darwin') {
		const allowed = (kind: string): boolean => snapshot.permissions.some(permission => permission.kind === kind && permission.status === 'granted');
		await engine.execute({ type: 'reportNativePermissions', screen: allowed('screen'), input: allowed('accessibility'), camera: allowed('camera'), microphone: allowed('microphone') });
		if (allowed('accessibility')) {
			await engine.execute({ type: 'enableNativeInput' });
		}
	} else if (platform === 'win32' || platform === 'linux') {
		await engine.execute({ type: 'enableNativeInput' });
	}
}

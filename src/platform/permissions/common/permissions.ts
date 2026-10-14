export const permissionKinds = ['screen', 'accessibility', 'camera', 'microphone'] as const;
export type PermissionKind = typeof permissionKinds[number];
export type PermissionStatus = 'granted' | 'denied' | 'restricted' | 'not-determined' | 'unknown' | 'unsupported';
export interface MachinePermission {
	readonly kind: PermissionKind;
	readonly status: PermissionStatus;
	readonly canRequest: boolean;
	readonly canOpenSettings: boolean;
}
export interface PermissionSnapshot {
	readonly development: boolean;
	readonly permissions: readonly MachinePermission[];
}
export type PermissionCommand = { type: 'status' } | { type: 'request' | 'openSettings'; kind: PermissionKind };
function record(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}
export function isPermissionCommand(value: unknown): value is PermissionCommand {
	if (!record(value)) {
		return false;
	}
	return value.type === 'status' ? Object.keys(value).length === 1 :
		(value.type === 'request' || value.type === 'openSettings') && Object.keys(value).length === 2 && permissionKinds.some(kind => kind === value.kind);
}
export function isPermissionSnapshot(value: unknown): value is PermissionSnapshot {
	return record(value) && Object.keys(value).length === 2 && typeof value.development === 'boolean' && Array.isArray(value.permissions) && value.permissions.length === permissionKinds.length &&
		value.permissions.every((entry: unknown, index: number) => record(entry) && Object.keys(entry).length === 4 && entry.kind === permissionKinds[index] &&
			typeof entry.status === 'string' && ['granted', 'denied', 'restricted', 'not-determined', 'unknown', 'unsupported'].includes(entry.status) && typeof entry.canRequest === 'boolean' && typeof entry.canOpenSettings === 'boolean');
}

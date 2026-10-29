import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { Event } from '../../../base/common/event.js';
import type { PermissionCommand, PermissionSnapshot } from '../../../platform/permissions/common/permissions.js';

export const IPermissionService = createDecorator<IPermissionService>('permissionService');

export interface IPermissionService {
	readonly _serviceBrand: undefined;
	readonly onDidFocusWindow: Event<void>;
	execute(command: PermissionCommand): Promise<PermissionSnapshot>;
}

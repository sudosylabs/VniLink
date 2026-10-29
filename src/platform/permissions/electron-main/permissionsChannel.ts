import type { Event } from '../../../base/common/event.js';
import type { IServerChannel } from '../../../base/parts/ipc/common/ipc.js';
import { windowIdFromContext } from '../../ipc/electron-main/windowContext.js';
import { isPermissionCommand } from '../common/permissions.js';
import type { PermissionsMainService } from './permissionsMainService.js';

/** Serves live OS consent queries and fixed consent prompts to windows. */
export class PermissionsChannel implements IServerChannel {

	constructor(private readonly permissions: PermissionsMainService) { }

	public listen<T>(_context: string, event: string): Event<T> {
		throw new Error(`Unknown permissions event: ${event}`);
	}

	public async call<T>(context: string, command: string, arg?: unknown): Promise<T> {
		windowIdFromContext(context);
		if (command !== 'execute' || !isPermissionCommand(arg)) {
			throw new Error('Invalid permission command');
		}
		return await this.permissions.execute(arg) as T;
	}
}

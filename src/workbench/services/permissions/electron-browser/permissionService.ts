import { Disposable, toDisposable } from '../../../../base/common/lifecycle.js';
import { Emitter } from '../../../../base/common/event.js';
import type { DesktopApi } from '../../../../platform/ipc/common/desktopApi.js';
import type { PermissionCommand, PermissionSnapshot } from '../../../../platform/permissions/common/permissions.js';
import type { IPermissionService } from '../common/permissionService.js';
export class PermissionService extends Disposable implements IPermissionService {
	declare public readonly _serviceBrand: undefined;
	private readonly focused = this._register(new Emitter<void>());
	public readonly onDidFocusWindow = this.focused.event;
	private wasFocused = false;
	constructor(private readonly api: Pick<DesktopApi, 'permissionCommand' | 'onWindowStateChanged'>) {
		super(); this._register(toDisposable(api.onWindowStateChanged(state => {
			if (state.focused && !this.wasFocused) {
				this.focused.fire();
			}
			this.wasFocused = state.focused;
		})));
	}
	public execute(command: PermissionCommand): Promise<PermissionSnapshot> {
		return this.api.permissionCommand(command);
	}
}

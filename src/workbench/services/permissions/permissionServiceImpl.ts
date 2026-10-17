import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';
import { IMainProcessService } from '../../../platform/ipc/common/mainProcessService.js';
import { INativeHostService } from '../../../platform/native/common/native.js';
import { isPermissionSnapshot, PERMISSIONS_CHANNEL_NAME, type PermissionCommand, type PermissionSnapshot } from '../../../platform/permissions/common/permissions.js';
import { IPermissionService } from './permissionService.js';

/** OS consent for this computer; re-read when the window regains focus after a settings visit. */
export class PermissionService extends Disposable implements IPermissionService {
	declare public readonly _serviceBrand: undefined;
	private readonly channel: IChannel;
	private readonly focused = this._register(new Emitter<void>());
	public readonly onDidFocusWindow = this.focused.event;
	private wasFocused = false;

	constructor(
		@IMainProcessService mainProcessService: IMainProcessService,
		@INativeHostService nativeHostService: INativeHostService,
	) {
		super();
		this.channel = mainProcessService.getChannel(PERMISSIONS_CHANNEL_NAME);
		this._register(nativeHostService.onDidChangeWindowState(state => {
			if (state.focused && !this.wasFocused) {
				this.focused.fire();
			}
			this.wasFocused = state.focused;
		}));
	}

	public async execute(command: PermissionCommand): Promise<PermissionSnapshot> {
		const snapshot = await this.channel.call<unknown>('execute', command);
		if (!isPermissionSnapshot(snapshot)) {
			throw new Error('Invalid permission snapshot');
		}
		return snapshot;
	}
}

registerSingleton(IPermissionService, PermissionService, InstantiationType.Delayed);

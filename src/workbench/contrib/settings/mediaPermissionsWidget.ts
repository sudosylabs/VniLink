import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Disposable, DisposableStore } from '../../../base/common/lifecycle.js';
import type { MachinePermission } from '../../../platform/permissions/common/permissions.js';
import { IPermissionService } from '../../services/permissions/permissionService.js';
import type { ISettingsWidget } from './settingsLayout.js';

/** Offers camera and microphone access when this app does not have it yet. */
export class MediaPermissionsWidget extends Disposable implements ISettingsWidget {
	private readonly element = $('.settings-apply-row');
	private readonly message = $('p.settings-help');
	private readonly actions = this._register(new DisposableStore());
	private signature = '';

	constructor(@IPermissionService private readonly permissionService: IPermissionService) {
		super();
	}

	public render(container: HTMLElement): void {
		container.append(this.element, this.message);
	}

	public async refresh(): Promise<void> {
		let permissions: readonly MachinePermission[] = [];
		try {
			permissions = (await this.permissionService.execute({ type: 'status' })).permissions;
		} catch {
			// Access & Security reports system access it cannot check.
		}
		if (this.isDisposed()) {
			return;
		}
		const missing = permissions.filter(permission => (permission.kind === 'camera' || permission.kind === 'microphone') && permission.status !== 'granted' && (permission.canRequest || permission.canOpenSettings));
		const signature = JSON.stringify(missing);
		if (signature === this.signature) {
			return;
		}
		this.signature = signature;
		this.actions.clear();
		this.element.replaceChildren();
		for (const permission of missing) {
			const action = append(this.element, $<HTMLButtonElement>('button.settings-text-button', { type: 'button' }));
			action.textContent = permission.kind === 'camera' ? nls.localize('media.cameraAccess', 'Allow camera access') : nls.localize('media.microphoneAccess', 'Allow microphone access');
			this.actions.add(addDisposableListener(action, EventType.CLICK, () => {
				void this.request(permission);
			}));
		}
	}

	private async request(permission: MachinePermission): Promise<void> {
		this.message.textContent = '';
		try {
			await this.permissionService.execute({ type: permission.canRequest ? 'request' : 'openSettings', kind: permission.kind });
			await this.refresh();
		} catch {
			if (!this.isDisposed()) {
				this.message.textContent = nls.localize('media.permissionFailed', 'Could not request system access. Check permissions in Access & Security.');
			}
		}
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}

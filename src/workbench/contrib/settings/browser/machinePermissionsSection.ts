import { appendAccessSection } from './accessSection.js';
import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import { IPermissionService } from '../../../services/permissions/common/permissionService.js';
import { permissionKinds, type MachinePermission, type PermissionCommand, type PermissionKind, type PermissionSnapshot, type PermissionStatus } from '../../../../platform/permissions/common/permissions.js';

const titles: Record<PermissionKind, string> = {
	screen: nls.localize('permissions.screen', 'Screen Recording'), accessibility: nls.localize('permissions.accessibility', 'Accessibility'), camera: nls.localize('permissions.camera', 'Camera'), microphone: nls.localize('permissions.microphone', 'Microphone'),
};

const descriptions: Record<PermissionKind, string> = {
	screen: nls.localize('permissions.screenHelp', 'Capture this computer’s screen. Changes in System Settings may require restarting the app.'),
	accessibility: nls.localize('permissions.accessibilityHelp', 'Send keyboard and mouse input to this computer.'),
	camera: nls.localize('permissions.cameraHelp', 'Access cameras connected to this computer.'),
	microphone: nls.localize('permissions.microphoneHelp', 'Capture microphone input. Changes in System Settings may require restarting the app.'),
};

const statuses: Record<PermissionStatus, string> = {
	granted: nls.localize('permissions.granted', 'Allowed'), denied: nls.localize('permissions.denied', 'Not allowed'), restricted: nls.localize('permissions.restricted', 'Restricted'), 'not-determined': nls.localize('permissions.notDetermined', 'Not requested'), unknown: nls.localize('permissions.unknown', 'Status unavailable'), unsupported: nls.localize('permissions.unsupported', 'Not exposed by this platform'),
};

interface PermissionRow { status: HTMLElement; button: HTMLButtonElement; }

export class MachinePermissionsSection extends Disposable {
	public readonly element = document.createElement('section');
	private readonly rows = new Map<PermissionKind, PermissionRow>();
	private readonly refresh = document.createElement('button');
	private readonly message = document.createElement('p');
	private readonly development = document.createElement('p');
	private snapshot: PermissionSnapshot | undefined;
	private busy = false;

	constructor(@IPermissionService private readonly permissions: IPermissionService) {
		super();
		this.element.className = 'access-section';
		const header = document.createElement('div');
		header.className = 'access-section-header';
		const title = document.createElement('h2');
		title.textContent = nls.localize('permissions.heading', 'Permissions on this computer');
		this.refresh.type = 'button';
		this.refresh.className = 'access-text-button';
		this.refresh.textContent = nls.localize('access.refresh', 'Refresh');
		header.append(title, this.refresh);
		const help = document.createElement('p');
		help.className = 'access-description';
		help.textContent = nls.localize('permissions.help', 'Operating-system access for this app. Allowing access here does not give a paired device permission to use it.');
		const controlsBody = document.createElement('div');
		appendAccessSection(this.element, header, help, [controlsBody]);
		for (const kind of permissionKinds) {
			const row = document.createElement('div');
			row.className = 'access-toggle-row';
			row.dataset.permissionKind = kind;
			const text = document.createElement('div');
			const label = document.createElement('span');
			label.className = 'access-toggle-title';
			label.textContent = titles[kind];
			const description = document.createElement('span');
			description.className = 'access-description';
			description.textContent = descriptions[kind];
			text.append(label, description);
			const controls = document.createElement('div');
			controls.className = 'machine-permission-controls';
			const status = document.createElement('span');
			status.className = 'machine-permission-status';
			status.textContent = nls.localize('permissions.checking', 'Checking…');
			const button = document.createElement('button');
			button.type = 'button';
			button.className = 'access-text-button';
			button.hidden = true;
			controls.append(status, button);
			row.append(text, controls);
			controlsBody.append(row);
			this.rows.set(kind, { status, button });
			this._register(addDisposableListener(button, 'click', () => {
				const permission = this.snapshot?.permissions.find(permission => permission.kind === kind);
				if (!this.busy && permission) {
					void this.run({ type: permission.canRequest ? 'request' : 'openSettings', kind });
				}
			}));
		}
		this.development.className = 'access-description';
		this.development.hidden = true;
		this.development.textContent = nls.localize('permissions.development', 'Development builds appear as Electron in system settings.');
		this.message.className = 'access-status';
		this.message.setAttribute('role', 'status');
		controlsBody.append(this.development, this.message);
		this._register(addDisposableListener(this.refresh, 'click', () => {
			void this.run({ type: 'status' });
		}));
		this._register(permissions.onDidFocusWindow(() => {
			void this.run({ type: 'status' });
		}));
		void this.run({ type: 'status' });
	}

	private disposed(): boolean {
		return this._store.isDisposed;
	}

	private render(permission: MachinePermission): void {
		const row = this.rows.get(permission.kind);
		if (!row) {
			return;
		}
		row.status.textContent = statuses[permission.status];
		row.status.dataset.status = permission.status;
		row.button.hidden = !permission.canRequest && !permission.canOpenSettings;
		row.button.textContent = permission.canRequest ? nls.localize('permissions.request', 'Request access') : nls.localize('permissions.settings', 'Open settings');
		row.button.setAttribute('aria-label', nls.localize('permissions.actionLabel', '{0}: {1}', titles[permission.kind], row.button.textContent));
	}

	private async run(command: PermissionCommand): Promise<void> {
		if (this.busy || this.disposed()) {
			return;
		}
		this.busy = true;
		this.element.setAttribute('aria-busy', 'true');
		this.refresh.disabled = true;
		for (const row of this.rows.values()) {
			row.button.disabled = true;
		}
		try {
			const snapshot = await this.permissions.execute(command);
			if (!this.disposed()) {
				this.snapshot = snapshot;
				for (const permission of snapshot.permissions) {
					this.render(permission);
				}
				this.development.hidden = !snapshot.development;
				this.message.textContent = '';
			}
		} catch {
			if (!this.disposed()) {
				this.snapshot = undefined;
				for (const row of this.rows.values()) {
					row.status.textContent = statuses.unknown;
					row.status.dataset.status = 'unknown';
				}
				this.message.textContent = nls.localize('permissions.failed', 'Could not check or change OS access. Refresh to check the current status.');
			}
		} finally {
			if (!this.disposed()) {
				this.busy = false;
				this.element.setAttribute('aria-busy', 'false');
				this.refresh.disabled = false;
				for (const row of this.rows.values()) {
					row.button.disabled = !this.snapshot;
				}
			}
		}
	}
}

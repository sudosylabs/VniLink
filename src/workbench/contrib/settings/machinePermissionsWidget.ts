import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { permissionKinds, type MachinePermission, type PermissionCommand, type PermissionKind, type PermissionSnapshot, type PermissionStatus } from '../../../platform/permissions/common/permissions.js';
import { IPermissionService } from '../../services/permissions/permissionService.js';
import type { ISettingsWidget } from './settingsLayout.js';

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

interface PermissionRow { readonly status: HTMLElement; readonly button: HTMLButtonElement; }

/** This app's operating-system access: live consent with request actions, not a setting. */
export class MachinePermissionsWidget extends Disposable implements ISettingsWidget {
	private readonly element = $('section.access-section');
	private readonly rows = new Map<PermissionKind, PermissionRow>();
	private readonly message = $('p.access-status', { role: 'status' });
	private snapshot: PermissionSnapshot | undefined;
	private busy = false;

	constructor(@IPermissionService private readonly permissionService: IPermissionService) {
		super();
	}

	public render(container: HTMLElement): void {
		const intro = append(this.element, $('.access-section-intro'));
		append(append(intro, $('.access-section-header')), $('h2')).textContent = nls.localize('permissions.heading', 'Permissions on this computer');
		append(intro, $('p.access-description')).textContent = nls.localize('permissions.help', 'Operating-system access for this app. Allowing access here does not give a paired device permission to use it.');
		const body = append(append(this.element, $('.access-section-body')), $('div'));
		for (const kind of permissionKinds) {
			const row = append(body, $('.access-toggle-row', { 'data-permission-kind': kind }));
			const text = append(row, $('div'));
			append(text, $('span.access-toggle-title')).textContent = titles[kind];
			append(text, $('span.access-description')).textContent = descriptions[kind];
			const controls = append(row, $('.machine-permission-controls'));
			const status = append(controls, $('span.machine-permission-status'));
			status.textContent = nls.localize('permissions.checking', 'Checking…');
			const button = append(controls, $<HTMLButtonElement>('button.access-text-button', { type: 'button' }));
			button.hidden = true;
			this.rows.set(kind, { status, button });
			this._register(addDisposableListener(button, EventType.CLICK, () => {
				const permission = this.snapshot?.permissions.find(candidate => candidate.kind === kind);
				if (!this.busy && permission) {
					void this.run({ type: permission.canRequest ? 'request' : 'openSettings', kind });
				}
			}));
		}
		body.append(this.message);
		container.append(this.element);
	}

	public refresh(): Promise<void> {
		return this.run({ type: 'status' });
	}

	private renderPermission(permission: MachinePermission): void {
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
		if (this.busy || this.isDisposed()) {
			return;
		}
		this.busy = true;
		if (command.type !== 'status' || !this.snapshot) {
			this.element.setAttribute('aria-busy', 'true');
			for (const row of this.rows.values()) {
				row.button.disabled = true;
			}
		}
		try {
			const snapshot = await this.permissionService.execute(command);
			if (!this.isDisposed()) {
				this.snapshot = snapshot;
				for (const permission of snapshot.permissions) {
					this.renderPermission(permission);
				}
				this.message.textContent = '';
			}
		} catch {
			if (!this.isDisposed()) {
				this.snapshot = undefined;
				for (const row of this.rows.values()) {
					row.status.textContent = statuses.unknown;
					row.status.dataset.status = 'unknown';
				}
				this.message.textContent = nls.localize('permissions.failed', 'Could not check or change OS access. The status will be checked again automatically.');
			}
		} finally {
			if (!this.isDisposed()) {
				this.busy = false;
				this.element.setAttribute('aria-busy', 'false');
				for (const row of this.rows.values()) {
					row.button.disabled = !this.snapshot;
				}
			}
		}
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}

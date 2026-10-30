// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { isMacintosh } from '../../../base/common/platform.js';
import { permissionKinds, type MachinePermission, type PermissionCommand, type PermissionKind, type PermissionSnapshot, type PermissionStatus } from '../../../platform/permissions/common/permissions.js';
import { IPermissionService } from '../../services/permissions/permissionService.js';
import { renderSettingRow } from './settingRenderers.js';
import type { ISettingsWidget } from './settingsLayout.js';

const titles: Record<PermissionKind, string> = {
	screen: nls.localize('permissions.screen', 'Screen Recording'), accessibility: nls.localize('permissions.accessibility', 'Accessibility'), camera: nls.localize('permissions.camera', 'Camera'), microphone: nls.localize('permissions.microphone', 'Microphone'),
};

/** Only where the system's own name does not say what VniLink uses it for. */
const descriptions: Partial<Record<PermissionKind, string>> = {
	accessibility: nls.localize('permissions.accessibilityHelp', 'Needed to control this computer’s keyboard and mouse.'),
};

/** macOS applies some consent changes only after a restart; other systems have no such step. */
const restartNote = isMacintosh ? nls.localize('permissions.restartNote', ' Changes made in System Settings may need VniLink to restart.') : '';

const statuses: Record<PermissionStatus, string> = {
	granted: nls.localize('permissions.granted', 'Allowed'), denied: nls.localize('permissions.denied', 'Not allowed'), restricted: nls.localize('permissions.restricted', 'Restricted'), 'not-determined': nls.localize('permissions.notDetermined', 'Not requested'), unknown: nls.localize('permissions.unknown', 'Status unavailable'), unsupported: nls.localize('permissions.unsupported', 'Not used on this system'), 'not-required': nls.localize('permissions.notRequired', 'No permission needed'),
};

interface PermissionRow { readonly status: HTMLElement; readonly button: HTMLButtonElement; }

/** This app's operating-system access: live consent with request actions, not a setting. */
export class MachinePermissionsWidget extends Disposable implements ISettingsWidget {
	private readonly element = $('section', { 'data-section': 'permissions' });
	private readonly rows = new Map<PermissionKind, PermissionRow>();
	private readonly message = $('p.settings-help', { role: 'status' });
	private snapshot: PermissionSnapshot | undefined;
	private busy = false;

	constructor(@IPermissionService private readonly permissionService: IPermissionService) {
		super();
	}

	public render(container: HTMLElement): void {
		append(this.element, $('h2')).textContent = nls.localize('permissions.heading', 'Permissions on this computer');
		append(this.element, $('p.settings-help.settings-section-description')).textContent = nls.localize('permissions.help', 'What your system lets VniLink do. Allowing something here doesn’t share it.') + restartNote;
		for (const kind of permissionKinds) {
			const row = renderSettingRow(this.element, titles[kind], descriptions[kind]);
			row.element.dataset.permissionKind = kind;
			const status = append(row.control, $('span.setting-status'));
			status.textContent = nls.localize('permissions.checking', 'Checking…');
			const button = append(row.control, $<HTMLButtonElement>('button.settings-button', { type: 'button' }));
			button.hidden = true;
			this.rows.set(kind, { status, button });
			this._register(addDisposableListener(button, EventType.CLICK, () => {
				const permission = this.snapshot?.permissions.find(candidate => candidate.kind === kind);
				if (!this.busy && permission) {
					void this.run({ type: permission.canRequest ? 'request' : 'openSettings', kind });
				}
			}));
		}
		this.element.append(this.message);
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
		// Allowed access needs no action here; the system's own settings can still revoke it.
		row.button.hidden = permission.status === 'granted' || (!permission.canRequest && !permission.canOpenSettings);
		row.button.textContent = permission.canRequest ? nls.localize('permissions.request', 'Request access')
			: isMacintosh ? nls.localize('permissions.systemSettings', 'Open System Settings') : nls.localize('permissions.settings', 'Open Settings');
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

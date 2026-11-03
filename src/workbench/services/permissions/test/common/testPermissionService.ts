// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter } from '../../../../../base/common/event.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import { permissionKinds, type PermissionCommand, type PermissionSnapshot } from '../../../../../platform/permissions/common/permissions.js';
import type { IPermissionService } from '../../permissionService.js';

/**
 * An {@link IPermissionService} for a computer where the camera can be
 * requested, the screen is denied and nothing else applies. A request grants
 * the camera; `fail` makes every query fail. Every command is kept in
 * `commands`, and `focus` stands for the window gaining focus.
 */
export class TestPermissionService extends Disposable implements IPermissionService {
	declare public readonly _serviceBrand: undefined;
	private readonly focused = this._register(new Emitter<void>());
	public readonly onDidFocusWindow = this.focused.event;
	public readonly commands: PermissionCommand[] = [];
	public fail = false;
	public cameraAllowed = false;

	public focus(): void {
		this.focused.fire();
	}

	public execute(command: PermissionCommand): Promise<PermissionSnapshot> {
		this.commands.push(command);
		if (this.fail) {
			return Promise.reject(new Error('OS query failed'));
		}
		if (command.type === 'request') {
			this.cameraAllowed = true;
		}
		return Promise.resolve({
			development: true,
			permissions: permissionKinds.map(kind => ({
				kind,
				status: kind === 'camera' ? (this.cameraAllowed ? 'granted' : 'not-determined') : kind === 'screen' ? 'denied' : 'unsupported',
				canRequest: kind === 'camera' && !this.cameraAllowed,
				canOpenSettings: kind === 'camera' || kind === 'screen',
			})),
		});
	}
}

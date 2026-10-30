// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { mainWindow } from '../../../base/browser/window.js';
import { ICommandService } from '../../../platform/commands/common/commands.js';
import { IWorkbenchConfigurationService } from '../configuration/configurationService.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import type { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { IMainProcessService } from '../../../platform/ipc/common/mainProcessService.js';
import { INativeHostService } from '../../../platform/native/common/native.js';
import { IPermissionService } from '../permissions/permissionService.js';

/**
 * Automation access to the window's real services, in the role of VS Code's
 * `BrowserWindowDriver`. Only registered for isolated smoke-test launches.
 */
export class BrowserWindowDriver {

	constructor(
		@IEngineService public readonly engine: IEngineService,
		@IWorkbenchConfigurationService public readonly configuration: IWorkbenchConfigurationService,
		@INativeHostService public readonly nativeHost: INativeHostService,
		@IPermissionService public readonly permissions: IPermissionService,
		@IMainProcessService public readonly mainProcess: IMainProcessService,
		@ICommandService public readonly commands: ICommandService,
	) { }
}

export function registerWindowDriver(instantiationService: IInstantiationService): void {
	Object.assign(mainWindow, { driver: instantiationService.createInstance(BrowserWindowDriver) });
}

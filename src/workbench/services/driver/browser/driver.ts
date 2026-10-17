import { mainWindow } from '../../../../base/browser/window.js';
import { IConfigurationService } from '../../../../platform/configuration/common/configuration.js';
import { IEngineService } from '../../../../platform/engine/common/engineService.js';
import type { IInstantiationService } from '../../../../platform/instantiation/common/instantiation.js';
import { IMainProcessService } from '../../../../platform/ipc/common/mainProcessService.js';
import { INativeHostService } from '../../../../platform/native/common/native.js';
import { IPermissionService } from '../../permissions/common/permissionService.js';

/**
 * Automation access to the window's real services, in the role of VS Code's
 * `BrowserWindowDriver`. Only registered for isolated smoke-test launches.
 */
export class BrowserWindowDriver {

	constructor(
		@IEngineService public readonly engine: IEngineService,
		@IConfigurationService public readonly configuration: IConfigurationService,
		@INativeHostService public readonly nativeHost: INativeHostService,
		@IPermissionService public readonly permissions: IPermissionService,
		@IMainProcessService public readonly mainProcess: IMainProcessService,
	) { }
}

export function registerWindowDriver(instantiationService: IInstantiationService): void {
	Object.assign(mainWindow, { driver: instantiationService.createInstance(BrowserWindowDriver) });
}

import { refineServiceDecorator } from '../../../platform/instantiation/common/instantiation.js';
import { IConfigurationService } from '../../../platform/configuration/common/configuration.js';

export const IWorkbenchConfigurationService = refineServiceDecorator<IConfigurationService, IWorkbenchConfigurationService>(IConfigurationService);

/**
 * VS Code's configuration contract with what VniLink's settings add: a setting
 * owned by the engine can be unreadable while the engine is unavailable, and
 * some settings are not supported on every computer.
 */
export interface IWorkbenchConfigurationService extends IConfigurationService {
	/** Resolves once the window holds the main process's settings. */
	initialize(): Promise<void>;

	/** Why `key` cannot be read or changed now, or `undefined` when it can. */
	getUnavailableReason(key: string): string | undefined;

	/** Whether `key` is readable but not supported on this computer. */
	isRestricted(key: string): boolean;
}

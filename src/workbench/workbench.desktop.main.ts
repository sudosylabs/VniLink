// The desktop workbench: every service and contribution it is composed of.
// Importing a module registers it; desktop.main.ts creates the window's core services.

//#region Services

import { InstantiationType, registerSingleton } from '../platform/instantiation/common/extensions.js';
import { IContextKeyService } from '../platform/contextkey/common/contextkey.js';
import { ContextKeyService } from '../platform/contextkey/browser/contextKeyService.js';
import { IMenuService } from '../platform/actions/common/actions.js';
import { MenuService } from '../platform/actions/common/menuService.js';
import '../platform/recording/electron-browser/recordingService.js';
import '../platform/theme/browser/themeService.js';
import './services/application/applicationServiceImpl.js';
import './services/commands/commandService.js';
import './services/configuration/configurationServiceImpl.js';
import './services/connection/connectionServiceImpl.js';
import './services/devices/deviceServiceImpl.js';
import './services/host/nativeHostService.js';
import './services/lifecycle/lifecycleService.js';
import './services/nativeRuntime/nativeRuntimeServiceImpl.js';
import './services/permissions/permissionServiceImpl.js';
import './services/remoteDesktop/remoteDesktopServiceImpl.js';
import './services/screenSnapshot/screenSnapshotServiceImpl.js';
import './services/tabs/tabServiceImpl.js';

registerSingleton(IContextKeyService, ContextKeyService, InstantiationType.Delayed);

registerSingleton(IMenuService, MenuService, InstantiationType.Delayed);

//#endregion

//#region Contributions

import { registerWorkbenchContribution2, WorkbenchPhase } from './contributions.js';
import { WorkbenchContextKeysHandler } from './contextkeysHandler.js';
import './actions/applicationActions.js';
import './contrib/devices/devices.contribution.js';
import './contrib/settings/settings.contribution.js';
import './contrib/home/home.contribution.js';

registerWorkbenchContribution2(WorkbenchContextKeysHandler.ID, WorkbenchContextKeysHandler, WorkbenchPhase.BlockStartup);

//#endregion

export { main } from './desktop.main.js';

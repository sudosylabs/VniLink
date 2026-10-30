// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import './media/remoteDesktop.css';
import { localize2 } from '../../../nls.js';
import { MenuRegistry, registerAction2 } from '../../../platform/actions/common/actions.js';
import { EngineAvailableContext } from '../../contextkeys.js';
import { ExportDiagnosticsAction, OpenLogsFolderAction } from '../../actions/applicationActions.js';
import { LockComputerAction, RestartOrShutDownAction } from '../systemActions/systemActionsActions.js';
import { RemoteDesktopMenu } from './remoteDesktopMenu.js';
import { OpenRemoteDesktopAction, ViewScreenAction } from './remoteDesktopActions.js';

registerAction2(OpenRemoteDesktopAction);

registerAction2(ViewScreenAction);

// The computer on screen, then what explains a session, where a picture's problems are seen.
MenuRegistry.appendMenuItem(RemoteDesktopMenu, { group: '2_system', order: 1, command: { id: LockComputerAction.ID, title: localize2('desktop.lockComputer', 'Lock Screen') } });

MenuRegistry.appendMenuItem(RemoteDesktopMenu, { group: '2_system', order: 2, command: { id: RestartOrShutDownAction.ID, title: localize2('desktop.restartOrShutDown', 'Restart or Shut Down…') } });

MenuRegistry.appendMenuItem(RemoteDesktopMenu, { group: '4_diagnostics', order: 1, command: { id: ExportDiagnosticsAction.ID, title: localize2('app.exportDiagnostics', 'Export Diagnostics…'), precondition: EngineAvailableContext } });

MenuRegistry.appendMenuItem(RemoteDesktopMenu, { group: '4_diagnostics', order: 2, command: { id: OpenLogsFolderAction.ID, title: localize2('app.openLogsFolder', 'Open Logs Folder') } });

/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: core services for one native window and the workbench that uses them.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { mainWindow } from '../base/browser/window.js';
import { Disposable } from '../base/common/lifecycle.js';
import { IEngineService } from '../platform/engine/common/engineService.js';
import { EngineService } from '../platform/engine/electron-browser/engineService.js';
import { SyncDescriptor } from '../platform/instantiation/common/descriptors.js';
import { ServiceCollection } from '../platform/instantiation/common/serviceCollection.js';
import { IMainProcessService } from '../platform/ipc/common/mainProcessService.js';
import { ElectronIPCMainProcessService } from '../platform/ipc/electron-browser/mainProcessService.js';
import { URI } from '../base/common/uri.js';
import { joinPath } from '../base/common/resources.js';
import { ConsoleLogger, ILoggerService, ILogService } from '../platform/log/common/log.js';
import { LoggerChannelClient } from '../platform/log/common/logIpc.js';
import { LogService } from '../platform/log/common/logService.js';
import { INativeHostService } from '../platform/native/common/native.js';
import { NativeHostService } from '../platform/native/common/nativeHostService.js';
import type { INativeWindowConfiguration } from '../platform/window/common/windowConfiguration.js';
import { Workbench } from './workbench.js';

export class DesktopMain extends Disposable {

	constructor(private readonly configuration: INativeWindowConfiguration) {
		super();
	}

	public async open(): Promise<void> {
		const { serviceCollection, logService } = this.initServices();
		const workbench = this._register(new Workbench(mainWindow.document.body, this.configuration, serviceCollection, logService));
		await workbench.startup();
	}

	/** Creates the services that need this window's configuration; the rest register themselves. */
	private initServices(): { serviceCollection: ServiceCollection; logService: ILogService } {
		const serviceCollection = new ServiceCollection();
		const mainProcessService = this._register(new ElectronIPCMainProcessService(this.configuration.windowId));
		serviceCollection.set(IMainProcessService, mainProcessService);
		// As VS Code's window: its log goes through main to window<id>/renderer.log, and to the console.
		const { windowId, logLevel, logsPath } = this.configuration;
		const windowLogsPath = joinPath(URI.file(logsPath), `window${String(windowId)}`);
		const loggerService = this._register(new LoggerChannelClient(windowId, logLevel, windowLogsPath, [], mainProcessService.getChannel('logger')));
		serviceCollection.set(ILoggerService, loggerService);
		const fileLogger = this._register(loggerService.createLogger(joinPath(windowLogsPath, 'renderer.log'), { id: 'renderer', name: 'Window' }));
		const logService = this._register(new LogService(fileLogger, [new ConsoleLogger(logLevel)]));
		serviceCollection.set(ILogService, logService);
		serviceCollection.set(INativeHostService, new SyncDescriptor(NativeHostService, [this.configuration.windowId]));
		serviceCollection.set(IEngineService, new SyncDescriptor(EngineService, [this.configuration.engineAvailable]));
		return { serviceCollection, logService };
	}
}

export function main(configuration: INativeWindowConfiguration): Promise<void> {
	return new DesktopMain(configuration).open();
}

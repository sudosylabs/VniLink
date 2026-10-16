/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import { INativeRuntimeService } from '../services/nativeRuntime/common/nativeRuntimeService.js';
import { NativeRuntimeService } from '../services/nativeRuntime/electron-browser/nativeRuntimeService.js';
import { ITerminalSettingsService } from '../services/terminal/common/terminalSettingsService.js';
import { TerminalSettingsService } from '../services/terminal/electron-browser/terminalSettingsService.js';
import { IApplicationService } from '../services/application/common/applicationService.js';
import { ApplicationService } from '../services/application/electron-browser/applicationService.js';
import { IRemoteDesktopService } from '../services/remoteDesktop/common/remoteDesktopService.js';
import { RemoteDesktopService } from '../services/remoteDesktop/electron-browser/remoteDesktopService.js';
import { IPermissionService } from '../services/permissions/common/permissionService.js';
import { PermissionService } from '../services/permissions/electron-browser/permissionService.js';
import '../services/tabs/common/tabServiceImpl.js';
import '../../platform/theme/browser/themeService.js';
import '../services/host/electron-browser/nativeHostService.js';
import { ServiceCollection } from '../../platform/instantiation/common/serviceCollection.js';
import { SyncDescriptor } from '../../platform/instantiation/common/descriptors.js';
import { Disposable } from '../../base/common/lifecycle.js';
import { Workbench } from '../browser/workbench.js';
import { NativeTitlebarPart } from './parts/titlebar/titlebarPart.js';
import { IMainProcessService } from '../../platform/ipc/common/mainProcessService.js';
import { MainProcessService } from '../../platform/ipc/electron-browser/mainProcessService.js';
import { INativeHostService } from '../../platform/native/common/native.js';
import { NativeHostService } from '../../platform/native/common/nativeHostService.js';
import type { DesktopApi, AppInfo } from '../../platform/ipc/common/desktopApi.js';
import { IDeviceService } from '../services/devices/common/deviceService.js';
import { DeviceService } from '../services/devices/electron-browser/deviceService.js';
import { IConfigurationService } from '../../platform/configuration/common/configuration.js';
import { ConfigurationService } from '../../platform/configuration/electron-browser/configurationService.js';
import { IConnectionService } from '../services/connection/common/connectionService.js';
import { ConnectionService } from '../services/connection/electron-browser/connectionService.js';
import { IAccessService } from '../services/access/common/accessService.js';
import { AccessService } from '../services/access/electron-browser/accessService.js';

export class DesktopMain extends Disposable {
	private opened: Promise<void> | undefined;

	constructor(private readonly api: DesktopApi, private readonly info: AppInfo) {
		super();
	}

	public open(): Promise<void> {
		this.opened ??= this.createWorkbench();
		return this.opened;
	}

	private async createWorkbench(): Promise<void> {
		if (this._store.isDisposed) {
			throw new Error('Desktop main disposed');
		}
		const workbench = this._register(new Workbench(document.body, this.info, this.initServices(), (services, onError) => services.createInstance(NativeTitlebarPart, this.info.osRelease, onError)));
		try {
			await workbench.startup();
		} catch (error) {
			this.dispose();
			throw error;
		}
	}

	private initServices(): ServiceCollection {
		const api = this.api;
		const info = this.info;
		const services = new ServiceCollection(
			[IMainProcessService, new SyncDescriptor(MainProcessService, [api, info.windowId])],
			[INativeHostService, new SyncDescriptor(NativeHostService, [info.windowId])],
			[IApplicationService, new SyncDescriptor(ApplicationService, [api])],
			[ITerminalSettingsService, new SyncDescriptor(TerminalSettingsService, [api, info.engineConnected])],
			[INativeRuntimeService, new SyncDescriptor(NativeRuntimeService, [api, info.engineConnected])],
			[IRemoteDesktopService, new SyncDescriptor(RemoteDesktopService, [api, info.engineConnected])],
			[IPermissionService, new SyncDescriptor(PermissionService, [api])],
			[IAccessService, new SyncDescriptor(AccessService, [api, info.engineConnected])],
			[IConnectionService, new SyncDescriptor(ConnectionService, [api, info.engineConnected])],
			[IDeviceService, new SyncDescriptor(DeviceService, [api, info.engineConnected])],
			[IConfigurationService, new SyncDescriptor(ConfigurationService, [api])],
		);
		return services;
	}
}

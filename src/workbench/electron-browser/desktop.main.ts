/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import { Disposable } from '../../base/common/lifecycle.js';
import { IConfigurationService } from '../../platform/configuration/common/configuration.js';
import { ConfigurationService } from '../../platform/configuration/electron-browser/configurationService.js';
import { IEngineService } from '../../platform/engine/common/engineService.js';
import { EngineService } from '../../platform/engine/electron-browser/engineService.js';
import { SyncDescriptor } from '../../platform/instantiation/common/descriptors.js';
import { ServiceCollection } from '../../platform/instantiation/common/serviceCollection.js';
import { IMainProcessService } from '../../platform/ipc/common/mainProcessService.js';
import { ElectronIPCMainProcessService } from '../../platform/ipc/electron-browser/mainProcessService.js';
import { INativeHostService } from '../../platform/native/common/native.js';
import { NativeHostService } from '../../platform/native/common/nativeHostService.js';
import '../../platform/recording/electron-browser/recordingService.js';
import type { INativeWindowConfiguration } from '../../platform/window/common/windowConfiguration.js';
import { IAccessService } from '../services/access/common/accessService.js';
import { AccessService } from '../services/access/electron-browser/accessService.js';
import { IApplicationService } from '../services/application/common/applicationService.js';
import { ApplicationService } from '../services/application/electron-browser/applicationService.js';
import { IConnectionService } from '../services/connection/common/connectionService.js';
import { ConnectionService } from '../services/connection/electron-browser/connectionService.js';
import { IDeviceService } from '../services/devices/common/deviceService.js';
import { DeviceService } from '../services/devices/electron-browser/deviceService.js';
import '../services/host/electron-browser/nativeHostService.js';
import { INativeRuntimeService } from '../services/nativeRuntime/common/nativeRuntimeService.js';
import { NativeRuntimeService } from '../services/nativeRuntime/electron-browser/nativeRuntimeService.js';
import { IPermissionService } from '../services/permissions/common/permissionService.js';
import { PermissionService } from '../services/permissions/electron-browser/permissionService.js';
import { IRemoteDesktopService } from '../services/remoteDesktop/common/remoteDesktopService.js';
import { RemoteDesktopService } from '../services/remoteDesktop/electron-browser/remoteDesktopService.js';
import { IScreenSnapshotService } from '../services/screenSnapshot/common/screenSnapshotService.js';
import { ScreenSnapshotService } from '../services/screenSnapshot/electron-browser/screenSnapshotService.js';
import '../services/tabs/common/tabServiceImpl.js';
import { ITerminalSettingsService } from '../services/terminal/common/terminalSettingsService.js';
import { TerminalSettingsService } from '../services/terminal/electron-browser/terminalSettingsService.js';
import '../../platform/theme/browser/themeService.js';
import { Workbench } from '../browser/workbench.js';
import { NativeTitlebarPart } from './parts/titlebar/titlebarPart.js';

export class DesktopMain extends Disposable {
	private opened: Promise<void> | undefined;

	constructor(private readonly configuration: INativeWindowConfiguration) {
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
		const workbench = this._register(new Workbench(document.body, this.configuration, this.initServices(), (services, onError) => services.createInstance(NativeTitlebarPart, this.configuration.osRelease, onError)));
		try {
			await workbench.startup();
		} catch (error) {
			this.dispose();
			throw error;
		}
	}

	private initServices(): ServiceCollection {
		const configuration = this.configuration;
		const mainProcessService = this._register(new ElectronIPCMainProcessService(configuration.windowId));
		return new ServiceCollection(
			[IMainProcessService, mainProcessService],
			[INativeHostService, new SyncDescriptor(NativeHostService, [configuration.windowId])],
			[IEngineService, new SyncDescriptor(EngineService, [configuration.engineAvailable])],
			[IConfigurationService, new SyncDescriptor(ConfigurationService)],
			[IApplicationService, new SyncDescriptor(ApplicationService)],
			[ITerminalSettingsService, new SyncDescriptor(TerminalSettingsService)],
			[INativeRuntimeService, new SyncDescriptor(NativeRuntimeService)],
			[IRemoteDesktopService, new SyncDescriptor(RemoteDesktopService)],
			[IPermissionService, new SyncDescriptor(PermissionService)],
			[IAccessService, new SyncDescriptor(AccessService)],
			[IConnectionService, new SyncDescriptor(ConnectionService)],
			[IDeviceService, new SyncDescriptor(DeviceService)],
			[IScreenSnapshotService, new SyncDescriptor(ScreenSnapshotService)],
		);
	}
}

/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import '../../platform/theme/browser/themeService.js';
import '../services/host/electron-browser/nativeHostService.js';
import { InstantiationService } from '../../platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../platform/instantiation/common/serviceCollection.js';
import { SyncDescriptor } from '../../platform/instantiation/common/descriptors.js';
import { getSingletonServiceDescriptors } from '../../platform/instantiation/common/extensions.js';
import { IMainProcessService } from '../../platform/ipc/common/mainProcessService.js';
import { MainProcessService } from '../../platform/ipc/electron-browser/mainProcessService.js';
import { INativeHostService } from '../../platform/native/common/native.js';
import { NativeHostService } from '../../platform/native/common/nativeHostService.js';
import type { DesktopApi, AppInfo } from '../../platform/ipc/common/desktopApi.js';
export function createWorkbenchServices(api: DesktopApi, info: AppInfo): InstantiationService {
	const services = new ServiceCollection(
		[IMainProcessService, new SyncDescriptor(MainProcessService, [api, info.windowId])],
		[INativeHostService, new SyncDescriptor(NativeHostService, [info.windowId])],
	);
	for (const [id, descriptor] of getSingletonServiceDescriptors()) {
		services.set(id, descriptor);
	}
	return new InstantiationService(services, true);
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { registerAction2 } from '../../../platform/actions/common/actions.js';
import { SyncDescriptor } from '../../../platform/instantiation/common/descriptors.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { Registry } from '../../../platform/registry/common/platform.js';
import { Extensions, HomePageGroup, type IHomePagesRegistry } from '../home/homePages.js';
import { CreateInvitationAction, ImportInvitationAction } from './deviceActions.js';
import { DevicePairingPopover, IDevicePairingPopover } from './devicePairingPopover.js';
import { DevicesPage } from './devicesPage.js';

Registry.as<IHomePagesRegistry>(Extensions.HomePages).registerPage({
	id: 'devices',
	group: HomePageGroup.Devices,
	order: 0,
	title: nls.localize('home.devices', 'Devices'),
	icon: Icons.devices,
	ctorDescriptor: new SyncDescriptor(DevicesPage),
});

registerSingleton(IDevicePairingPopover, DevicePairingPopover, InstantiationType.Delayed);

registerAction2(ImportInvitationAction);

registerAction2(CreateInvitationAction);

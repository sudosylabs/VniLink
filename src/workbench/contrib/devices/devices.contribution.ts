import * as nls from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { SyncDescriptor } from '../../../platform/instantiation/common/descriptors.js';
import { Registry } from '../../../platform/registry/common/platform.js';
import { Extensions, HomePageGroup, type IHomePagesRegistry } from '../home/homePages.js';
import { DevicesPage } from './devicesPage.js';

Registry.as<IHomePagesRegistry>(Extensions.HomePages).registerPage({
	id: 'devices',
	group: HomePageGroup.Devices,
	order: 0,
	title: nls.localize('home.devices', 'Devices'),
	icon: Icons.devices,
	ctorDescriptor: new SyncDescriptor(DevicesPage),
});

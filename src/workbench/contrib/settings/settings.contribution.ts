import * as nls from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { SyncDescriptor } from '../../../platform/instantiation/common/descriptors.js';
import { Registry } from '../../../platform/registry/common/platform.js';
import { Extensions, HomePageGroup, type IHomePagesRegistry } from '../home/homePages.js';
import { AccessPage } from './accessPage.js';
import { AppearancePage } from './appearancePage.js';
import { ConnectionPage } from './connectionPage.js';
import { InputPage } from './inputPage.js';
import { MediaPage } from './mediaPage.js';
import { PowerPage } from './powerPage.js';
import { RecordingPage } from './recordingPage.js';
import { RemoteDesktopPage } from './remoteDesktopPage.js';
import { TerminalPage } from './terminalPage.js';

const registry = Registry.as<IHomePagesRegistry>(Extensions.HomePages);

const pages = [
	{ id: 'appearance', title: nls.localize('home.appearance', 'Appearance'), icon: Icons.appearance, ctorDescriptor: new SyncDescriptor(AppearancePage) },
	{ id: 'connection', title: nls.localize('home.connection', 'Connection'), icon: Icons.connection, ctorDescriptor: new SyncDescriptor(ConnectionPage) },
	{ id: 'access', title: nls.localize('home.access', 'Access & Security'), icon: Icons.security, ctorDescriptor: new SyncDescriptor(AccessPage) },
	{ id: 'desktop', title: nls.localize('home.desktop', 'Remote Desktop'), icon: Icons.remoteDesktop, ctorDescriptor: new SyncDescriptor(RemoteDesktopPage) },
	{ id: 'input', title: nls.localize('home.input', 'Keyboard & Mouse'), icon: Icons.keyboard, ctorDescriptor: new SyncDescriptor(InputPage) },
	{ id: 'terminal', title: nls.localize('home.terminal', 'Terminal'), icon: Icons.terminal, ctorDescriptor: new SyncDescriptor(TerminalPage) },
	{ id: 'media', title: nls.localize('home.media', 'Audio & Camera'), icon: Icons.audio, ctorDescriptor: new SyncDescriptor(MediaPage) },
	{ id: 'recording', title: nls.localize('home.recording', 'Recording'), icon: Icons.recording, ctorDescriptor: new SyncDescriptor(RecordingPage) },
	{ id: 'power', title: nls.localize('home.power', 'Power'), icon: Icons.power, ctorDescriptor: new SyncDescriptor(PowerPage) },
];

pages.forEach((page, index) => {
	registry.registerPage({ ...page, group: HomePageGroup.Settings, order: index });
});

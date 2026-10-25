import * as nls from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { SyncDescriptor } from '../../../platform/instantiation/common/descriptors.js';
import { Registry } from '../../../platform/registry/common/platform.js';
import { Extensions as HomeExtensions, HomePageGroup, type IHomePagesRegistry } from '../home/homePages.js';
import { CodecSettingControl } from './codecSettingControl.js';
import { DesktopQualityControl } from './desktopQualityControl.js';
import { MachinePermissionsWidget } from './machinePermissionsWidget.js';
import { MediaSourceControl } from './mediaSourceControl.js';
import { PointerSensitivityControl } from './pointerSensitivityControl.js';
import { PowerStatusWidget } from './powerStatusWidget.js';
import { RecordingFolderControl } from './recordingFolderControl.js';
import { RelaysSettingControl } from './relaysSettingControl.js';
import { RunningRelaysWidget } from './runningRelaysWidget.js';
import { Extensions as SettingExtensions, type ISettingControlsRegistry } from './settingRenderers.js';
import { SettingsRefresh, type ISettingsPageDescriptor } from './settingsLayout.js';
import { SettingsPage } from './settingsPage.js';
import { TerminalShellControl } from './terminalShellControl.js';
import { ThemeSettingControl } from './themeSettingControl.js';

const controls = Registry.as<ISettingControlsRegistry>(SettingExtensions.SettingControls);

controls.registerControl('appearance.theme', ThemeSettingControl);

controls.registerControl('connection.relays', RelaysSettingControl);

controls.registerControl('desktop.quality', DesktopQualityControl);

controls.registerControl('desktop.codec', CodecSettingControl);

controls.registerControl('input.pointerSensitivity', PointerSensitivityControl);

controls.registerControl('recording.directory', RecordingFolderControl);

controls.registerControl('terminal.shell', TerminalShellControl);

for (const key of ['media.camera', 'media.microphone', 'media.systemAudio']) {
	controls.registerControl(key, MediaSourceControl);
}

/** Pages in sidebar order. Every page but Connection shares one layout: sections of rows, divided by hairlines. */
const form = { contentClass: 'settings-form-content', fieldsClass: 'settings-form-fields', descriptionClass: 'settings-help' } as const;

const keepAwakeSettings = (scope: 'incoming' | 'outgoing'): string[] => [`power.${scope}.keepSystemAwake`, `power.${scope}.keepDisplayAwake`, `power.${scope}.idleTimeout`];

const pages: readonly ISettingsPageDescriptor[] = [
	{
		id: 'general',
		title: nls.localize('home.general', 'General'),
		description: nls.localize('general.description', 'How VniLink starts and stays reachable.'),
		icon: Icons.settings,
		...form,
		// Start at login is read from the operating system, which can change it too.
		refresh: SettingsRefresh.Interval,
		sections: [
			{ id: 'availability', settings: ['application.acceptConnections', 'application.openAtLogin', 'application.runInBackground'] },
		],
	},
	{
		id: 'appearance',
		title: nls.localize('home.appearance', 'Appearance'),
		description: nls.localize('appearance.description', 'Choose how VniLink looks.'),
		icon: Icons.appearance,
		...form,
		sections: [
			{ id: 'theme', title: nls.localize('appearance.theme', 'Theme'), settings: ['appearance.theme'] },
		],
	},
	{
		id: 'connection',
		title: nls.localize('home.connection', 'Connection'),
		description: nls.localize('home.connectionDescription', 'Relays and how your computers connect.'),
		icon: Icons.connection,
		contentClass: 'settings-form connection-form',
		descriptionClass: 'connection-description',
		sections: [{ id: 'relays', bare: true, settings: ['connection.relays'] }],
		footer: [{ id: 'running', bare: true, widget: new SyncDescriptor(RunningRelaysWidget) }],
	},
	{
		id: 'access',
		title: nls.localize('home.access', 'Access & Security'),
		description: nls.localize('access.description', 'What paired computers may use here, and what your system lets VniLink do.'),
		icon: Icons.security,
		...form,
		refresh: SettingsRefresh.Interval,
		sections: [
			{
				id: 'sharing',
				title: nls.localize('sharing.title', 'Sharing with paired computers'),
				description: nls.localize('sharing.description', 'Applies to every paired computer. Turning something off ends its use right away.'),
				settings: ['sharing.screen', 'sharing.files', 'sharing.terminal', 'sharing.camera', 'sharing.microphone', 'sharing.systemAudio', 'sharing.clipboard', 'sharing.recording', 'sharing.systemActions'],
			},
			{ id: 'permissions', bare: true, widget: new SyncDescriptor(MachinePermissionsWidget) },
			{
				id: 'security',
				title: nls.localize('security.local', 'Remote actions'),
				description: nls.localize('security.policyHelp', 'Used when System actions is on.'),
				settings: ['security.allowLock', 'security.allowRestart', 'security.allowShutdown', 'security.allowPrivacy'],
			},
		],
	},
	{
		id: 'desktop',
		title: nls.localize('home.desktop', 'Remote Desktop'),
		description: nls.localize('desktop.description', 'Defaults for the remote desktop sessions you start. The other computer may lower them.'),
		icon: Icons.remoteDesktop,
		className: 'remote-desktop-page',
		...form,
		sections: [
			{ id: 'quality', title: nls.localize('desktop.quality', 'Stream limits'), settings: ['desktop.quality'] },
			{ id: 'codec', title: nls.localize('desktop.codecTitle', 'Video codec'), settings: ['desktop.codec'] },
		],
	},
	{
		id: 'input',
		title: nls.localize('home.input', 'Keyboard & Mouse'),
		description: nls.localize('input.description', 'How your keyboard and mouse work on the computers you control.'),
		icon: Icons.keyboard,
		...form,
		sections: [
			{ id: 'keyboard', title: nls.localize('input.keyboard', 'Keyboard'), settings: ['input.keyboardMode', 'input.swapModifiers'] },
			{ id: 'mouse', title: nls.localize('input.mouse', 'Mouse'), settings: ['input.pointerSensitivity', 'input.reverseWheel', 'input.swapMouseButtons'] },
		],
	},
	{
		id: 'terminal',
		title: nls.localize('home.terminal', 'Terminal'),
		description: nls.localize('terminal.description', 'What paired computers get when they open a terminal on this computer.'),
		icon: Icons.terminal,
		...form,
		// The shell list is read again on return, so a shell installed meanwhile appears.
		refresh: SettingsRefresh.Focus,
		// Keeping detached shells and replaying their output wait for reconnecting terminals; their engine defaults stand.
		sections: [
			{ id: 'shell', settings: ['terminal.shell'] },
			{ id: 'sessions', title: nls.localize('terminal.sessions', 'Limits'), settings: ['terminal.maxSessions', 'terminal.maxSessionsPerDevice'] },
		],
	},
	{
		id: 'media',
		title: nls.localize('home.media', 'Audio & Camera'),
		description: nls.localize('media.description', 'The camera and sound this computer shares. Paired computers also need them turned on in Access & Security.'),
		icon: Icons.audio,
		...form,
		refresh: SettingsRefresh.Interval,
		sections: [
			{ id: 'sources', settings: ['media.camera', 'media.microphone', 'media.systemAudio'] },
		],
	},
	{
		id: 'recording',
		title: nls.localize('home.recording', 'Recording'),
		description: nls.localize('recording.description', 'Where recordings of remote desktop sessions go, and how large they grow.'),
		icon: Icons.recording,
		...form,
		sections: [
			{ id: 'destination', settings: ['recording.directory'] },
			{ id: 'files', title: nls.localize('recording.files', 'Files'), description: nls.localize('recording.filesHelp', 'A recording is split into files when one reaches either limit.'), settings: ['recording.maxSegmentSize', 'recording.maxSegmentDuration'] },
			{
				id: 'cleanup',
				title: nls.localize('recording.cleanup', 'Cleanup'),
				description: nls.localize('recording.cleanupHelp', 'The oldest files of a recording are deleted past either limit. Nothing else in the folder is touched.'),
				settings: ['recording.maxSegments', 'recording.maxTotalSize'],
			},
		],
	},
	{
		id: 'power',
		title: nls.localize('home.power', 'Power'),
		description: nls.localize('power.description', 'Keep computers awake during remote sessions.'),
		icon: Icons.power,
		...form,
		refresh: SettingsRefresh.Interval,
		sections: [
			{ id: 'incoming', title: nls.localize('power.incoming', 'While sharing this computer'), settings: keepAwakeSettings('incoming') },
			{ id: 'outgoing', title: nls.localize('power.outgoing', 'While controlling another computer'), settings: keepAwakeSettings('outgoing') },
			{ id: 'ending', title: nls.localize('power.ending', 'When remote control ends'), settings: ['power.lockAfterControl'] },
		],
		footer: [{ id: 'status', bare: true, widget: new SyncDescriptor(PowerStatusWidget) }],
	},
];

const homePages = Registry.as<IHomePagesRegistry>(HomeExtensions.HomePages);

pages.forEach((page, index) => {
	const ctorDescriptor = new SyncDescriptor(SettingsPage, [page]);
	homePages.registerPage({ id: page.id, group: HomePageGroup.Settings, order: index, title: page.title, icon: page.icon, ctorDescriptor });
});

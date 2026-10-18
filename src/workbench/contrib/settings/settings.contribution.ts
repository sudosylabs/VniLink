import * as nls from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { SyncDescriptor } from '../../../platform/instantiation/common/descriptors.js';
import { Registry } from '../../../platform/registry/common/platform.js';
import { Extensions as HomeExtensions, HomePageGroup, type IHomePagesRegistry } from '../home/homePages.js';
import { CodecSettingControl } from './codecSettingControl.js';
import { DesktopQualityControl } from './desktopQualityControl.js';
import { MachinePermissionsWidget } from './machinePermissionsWidget.js';
import { MediaPermissionsWidget } from './mediaPermissionsWidget.js';
import { MediaSourceControl } from './mediaSourceControl.js';
import { PointerSensitivityControl } from './pointerSensitivityControl.js';
import { PowerStatusWidget } from './powerStatusWidget.js';
import { RecordingFolderControl } from './recordingFolderControl.js';
import { RelaysSettingControl } from './relaysSettingControl.js';
import { RunningRelaysWidget } from './runningRelaysWidget.js';
import { Extensions as SettingExtensions, type ISettingControlsRegistry } from './settingRenderers.js';
import { SettingsPageStyle, type ISettingsPageDescriptor } from './settingsLayout.js';
import { SettingsPage } from './settingsPage.js';
import { ThemeSettingControl } from './themeSettingControl.js';

const controls = Registry.as<ISettingControlsRegistry>(SettingExtensions.SettingControls);

controls.registerControl('appearance.theme', ThemeSettingControl);

controls.registerControl('connection.relays', RelaysSettingControl);

controls.registerControl('desktop.quality', DesktopQualityControl);

controls.registerControl('desktop.codec', CodecSettingControl);

controls.registerControl('input.pointerSensitivity', PointerSensitivityControl);

controls.registerControl('recording.directory', RecordingFolderControl);

for (const key of ['media.camera', 'media.microphone', 'media.systemAudio']) {
	controls.registerControl(key, MediaSourceControl);
}

/** Pages in sidebar order. Form pages share one column layout; Access pairs each introduction with its controls. */
const form = { style: SettingsPageStyle.Form, contentClass: 'settings-form-content', fieldsClass: 'settings-form-fields', descriptionClass: 'settings-help' } as const;

const keepAwakeSettings = (scope: 'incoming' | 'outgoing'): string[] => [`power.${scope}.keepSystemAwake`, `power.${scope}.keepDisplayAwake`, `power.${scope}.idleTimeout`];

const pages: readonly ISettingsPageDescriptor[] = [
	{
		id: 'appearance',
		title: nls.localize('home.appearance', 'Appearance'),
		description: nls.localize('appearance.description', 'Make VniLink feel at home on your device.'),
		icon: Icons.appearance,
		style: SettingsPageStyle.Form,
		contentClass: 'appearance-form',
		descriptionClass: 'appearance-description',
		statusClass: 'appearance-status',
		sections: [
			{ id: 'theme', className: 'appearance-section', title: nls.localize('appearance.theme', 'Color theme'), description: nls.localize('appearance.themeDescription', 'Choose a look, or let it change with your system.'), settings: ['appearance.theme'] },
		],
	},
	{
		id: 'connection',
		title: nls.localize('home.connection', 'Connection'),
		description: nls.localize('home.connectionDescription', 'Relays and how your devices connect.'),
		icon: Icons.connection,
		style: SettingsPageStyle.Form,
		contentClass: 'settings-form connection-form',
		descriptionClass: 'connection-description',
		sections: [{ id: 'relays', bare: true, settings: ['connection.relays'] }],
		footer: [{ id: 'running', bare: true, widget: new SyncDescriptor(RunningRelaysWidget) }],
	},
	{
		id: 'access',
		title: nls.localize('home.access', 'Access & Security'),
		description: nls.localize('access.description', 'Manage the app’s system permissions and the actions remote devices may request.'),
		icon: Icons.security,
		style: SettingsPageStyle.Access,
		contentClass: 'access-content',
		descriptionClass: 'access-description',
		poll: true,
		sections: [
			{
				id: 'sharing',
				className: 'access-section',
				title: nls.localize('sharing.title', 'Sharing with paired computers'),
				description: nls.localize('sharing.description', 'These settings apply to every paired computer. Turning a service off stops its active use. System permissions still apply.'),
				settings: ['sharing.screen', 'sharing.files', 'sharing.terminal', 'sharing.camera', 'sharing.microphone', 'sharing.systemAudio', 'sharing.clipboard', 'sharing.recording', 'sharing.systemActions'],
			},
			{ id: 'permissions', bare: true, widget: new SyncDescriptor(MachinePermissionsWidget) },
			{
				id: 'security',
				className: 'access-section',
				title: nls.localize('security.local', 'Remote action rules'),
				description: nls.localize('security.policyHelp', 'Allow these requests from paired devices when System actions sharing is enabled. These switches set VniLink’s rules; they do not run an action or grant operating-system access.'),
				settings: ['security.allowLock', 'security.allowRestart', 'security.allowShutdown', 'security.allowPrivacy'],
			},
		],
	},
	{
		id: 'desktop',
		title: nls.localize('home.desktop', 'Remote Desktop'),
		description: nls.localize('desktop.description', 'Choose the picture quality and codec used for new remote desktop connections.'),
		icon: Icons.remoteDesktop,
		className: 'remote-desktop-page',
		style: SettingsPageStyle.Form,
		contentClass: 'desktop-content',
		descriptionClass: 'settings-help',
		sections: [
			{ id: 'quality', className: 'desktop-defaults-section', settings: ['desktop.quality'] },
			{ id: 'codec', className: 'desktop-processing-section', title: nls.localize('desktop.codecTitle', 'Video codec'), settings: ['desktop.codec'] },
		],
	},
	{
		id: 'input',
		title: nls.localize('home.input', 'Keyboard & Mouse'),
		description: nls.localize('input.description', 'Choose how your keyboard and mouse control remote computers. These defaults apply across your connections.'),
		icon: Icons.keyboard,
		...form,
		note: nls.localize('input.sessionNote', 'Applies to all connected computers and future connections. Release held keys and mouse buttons before changing these settings.'),
		sections: [
			{ id: 'keyboard', title: nls.localize('input.keyboard', 'Keyboard'), settings: ['input.keyboardMode', 'input.swapModifiers'] },
			{ id: 'mouse', title: nls.localize('input.mouse', 'Mouse'), settings: ['input.pointerSensitivity', 'input.reverseWheel', 'input.swapMouseButtons'] },
		],
	},
	{
		id: 'terminal',
		title: nls.localize('home.terminal', 'Terminal'),
		description: nls.localize('terminal.description', 'Limits for remote terminals hosted on this computer.'),
		icon: Icons.terminal,
		...form,
		sections: [
			{
				id: 'sessions',
				title: nls.localize('terminal.sessions', 'Session limits'),
				description: nls.localize('terminal.limitsHelp', 'A device needs Terminal permission and an available shell backend. Changing these limits does not launch a shell. Close hosted terminals before changing them.'),
				refreshable: true,
				settings: ['terminal.maxSessions', 'terminal.maxSessionsPerDevice'],
			},
			{ id: 'reconnect', title: nls.localize('terminal.reconnect', 'Disconnect & reconnect'), settings: ['terminal.persistence', 'terminal.detachedTimeout'] },
			{ id: 'history', title: nls.localize('terminal.history', 'Output history'), settings: ['terminal.historySize', 'terminal.replaySize'] },
		],
	},
	{
		id: 'media',
		title: nls.localize('home.media', 'Audio & Camera'),
		description: nls.localize('media.description', 'Choose which cameras and audio sources this computer can share.'),
		icon: Icons.audio,
		...form,
		note: nls.localize('media.note', 'Devices still need Camera, Microphone or System audio permission to use these sources.'),
		poll: true,
		header: [{ id: 'permissions', bare: true, widget: new SyncDescriptor(MediaPermissionsWidget) }],
		sections: [
			{ id: 'cameras', title: nls.localize('media.cameras', 'Cameras'), description: nls.localize('media.camerasHelp', 'Choose the camera this computer shares with permitted devices.'), settings: ['media.camera'] },
			{ id: 'microphones', title: nls.localize('media.microphones', 'Microphones'), description: nls.localize('media.microphonesHelp', 'Choose the microphone used when a permitted device requests it.'), settings: ['media.microphone'] },
			{ id: 'system', title: nls.localize('media.system', 'System audio'), description: nls.localize('media.systemHelp', 'Share this computer’s sound without changing its local speakers.'), settings: ['media.systemAudio'] },
		],
	},
	{
		id: 'recording',
		title: nls.localize('home.recording', 'Recording'),
		description: nls.localize('recording.description', 'Set the limits used when you record a remote desktop session.'),
		icon: Icons.recording,
		...form,
		note: nls.localize('recording.note', 'Used for new recordings. You can choose another destination when starting a recording.'),
		sections: [
			{ id: 'destination', title: nls.localize('recording.destination', 'Save recordings'), description: nls.localize('recording.destinationHelp', 'Each recording gets its own file in this folder.'), settings: ['recording.directory'] },
			{ id: 'limits', title: nls.localize('recording.limits', 'Recording segments'), description: nls.localize('recording.limitsHelp', 'A new segment begins when a limit is reached or the video format changes.'), settings: ['recording.maxSegmentSize', 'recording.maxSegmentDuration'] },
			{
				id: 'retention',
				title: nls.localize('recording.retention', 'Retention per recording'),
				description: nls.localize('recording.retentionHelp', 'Older completed segments from the same recording are removed when these limits are exceeded. Unrelated files are never part of this retention rule.'),
				settings: ['recording.maxSegments', 'recording.maxTotalSize'],
			},
		],
	},
	{
		id: 'power',
		title: nls.localize('home.power', 'Power'),
		description: nls.localize('power.description', 'Keep this computer awake while remote sessions are in use.'),
		icon: Icons.power,
		...form,
		note: nls.localize('power.note', 'Applies to this computer, including sessions already running.'),
		poll: true,
		sections: [
			{ id: 'incoming', title: nls.localize('power.incoming', 'While sharing this computer'), description: nls.localize('power.incomingHelp', 'Sleep behavior while another device is connected. Set the idle timeout to 0 to keep desktop sessions open.'), settings: keepAwakeSettings('incoming') },
			{ id: 'outgoing', title: nls.localize('power.outgoing', 'While controlling a remote computer'), description: nls.localize('power.outgoingHelp', 'Sleep behavior while you use another device. Set the idle timeout to 0 to keep desktop sessions open.'), settings: keepAwakeSettings('outgoing') },
			{ id: 'ending', title: nls.localize('power.ending', 'After remote control ends'), description: nls.localize('power.endingHelp', 'Protect the computer when the last controlling session finishes.'), settings: ['power.lockAfterControl'] },
		],
		footer: [{ id: 'status', bare: true, widget: new SyncDescriptor(PowerStatusWidget) }],
	},
];

const homePages = Registry.as<IHomePagesRegistry>(HomeExtensions.HomePages);

pages.forEach((page, index) => {
	const ctorDescriptor = new SyncDescriptor(SettingsPage, [page]);
	homePages.registerPage({ id: page.id, group: HomePageGroup.Settings, order: index, title: page.title, icon: page.icon, ctorDescriptor });
});

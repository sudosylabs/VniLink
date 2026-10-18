import * as nls from '../../../nls.js';
import type { IStringDictionary } from '../../../base/common/collections.js';
import { Registry } from '../../registry/common/platform.js';
import { isCodecPreference, isQualityLimits } from '../../engine/common/engine.js';
import { Extensions, type IConfigurationNode, type IConfigurationPropertySchema, type IConfigurationRegistry } from './configurationRegistry.js';

/**
 * Where a setting's value lives. `user` settings are stored in settings.json
 * and pushed to the engine; `system` settings are owned by the operating
 * system; every other source is a policy object the engine persists, read and
 * written as a whole.
 */
export type ConfigurationSource = 'user' | 'system' | 'power' | 'terminal' | 'sharing' | 'security' | 'relays';

export const configurationSources: readonly ConfigurationSource[] = ['user', 'system', 'power', 'terminal', 'sharing', 'security', 'relays'];

/** A registered setting, with VniLink's presentation and validation additions. */
export interface ISettingSchema extends IConfigurationPropertySchema {
	/** Suffix shown after a number control. */
	readonly unit?: string;
	/** Rules the JSON schema cannot express; returns a message when `value` is invalid. */
	readonly validate?: (value: unknown) => string | undefined;
	/** Why the setting cannot be changed when the snapshot lists it as restricted; defaults to "Not supported on this computer." */
	readonly restrictedReason?: string;
}

const settings = new Map<string, { readonly source: ConfigurationSource; readonly schema: ISettingSchema }>();

/** Engine-owned settings declare Rust's defaults; their effective values are always read from the engine. */
function registerSettings(source: ConfigurationSource, node: IConfigurationNode & { readonly properties: IStringDictionary<ISettingSchema> }): void {
	for (const [key, schema] of Object.entries(node.properties)) {
		settings.set(key, { source, schema });
	}
	Registry.as<IConfigurationRegistry>(Extensions.Configuration).registerConfiguration(node);
}

export function settingSource(key: string): ConfigurationSource | undefined {
	return settings.get(key)?.source;
}

export function settingSchema(key: string): ISettingSchema | undefined {
	return settings.get(key)?.schema;
}

export function settingKeys(source?: ConfigurationSource): string[] {
	return [...settings].filter(([, setting]) => source === undefined || setting.source === source).map(([key]) => key);
}

/** Returns why `value` is not valid for `key`, or `undefined` when it is. */
export function validateSetting(key: string, value: unknown): string | undefined {
	const schema = settingSchema(key);
	if (!schema) {
		return nls.localize('settings.unknown', 'Unknown setting {0}.', key);
	}
	switch (schema.type) {
		case 'boolean':
			if (typeof value !== 'boolean') {
				return nls.localize('settings.boolean', 'Choose on or off.');
			}
			break;
		case 'integer':
		case 'number':
			if (typeof value !== 'number' || !Number.isFinite(value) || (schema.type === 'integer' && !Number.isInteger(value))) {
				return schema.type === 'integer' ? nls.localize('settings.integer', 'Enter a whole number.') : nls.localize('settings.number', 'Enter a number.');
			}
			if ((schema.minimum !== undefined && value < schema.minimum) || (schema.maximum !== undefined && value > schema.maximum)) {
				return nls.localize('settings.range', 'Enter a value from {0} to {1}.', String(schema.minimum), String(schema.maximum));
			}
			break;
		case 'string':
			if (typeof value !== 'string' || (schema.enum && !schema.enum.includes(value))) {
				return nls.localize('settings.choice', 'Choose one of the available options.');
			}
			break;
		case 'array':
		case 'object':
		case 'null':
		case undefined:
			// Structured settings declare their own `validate`.
			break;
	}
	return schema.validate?.(value);
}

/** Every setting's default, keyed by setting. */
export function settingDefaults(source?: ConfigurationSource): Record<string, unknown> {
	return Object.fromEntries(settingKeys(source).map(key => [key, settingSchema(key)?.default]));
}

function minutes(value: unknown): string | undefined {
	return typeof value === 'number' && Number.isInteger(value * 60) ? undefined : nls.localize('settings.minutes', 'Enter whole seconds as minutes, for example 1.5.');
}

function kibibytes(value: unknown): string | undefined {
	return typeof value === 'number' && Number.isInteger(value * 1024) ? undefined : nls.localize('settings.kibibytes', 'Enter a size in whole bytes.');
}

function isAbsolutePath(path: string): boolean {
	return path.length > 0 && path.length <= 4096 && !path.includes('\0') && (path.startsWith('/') || /^[a-z]:[\\/]/i.test(path) || /^\\\\[^\\]+\\[^\\]+/.test(path));
}

export function isFileRoots(value: unknown): value is readonly string[] {
	return Array.isArray(value) && value.length <= 16 && new Set(value).size === value.length && value.every((path: unknown) => typeof path === 'string' && isAbsolutePath(path));
}

/** A media source id; the empty id turns the source off. */
function mediaSource(value: unknown): string | undefined {
	return typeof value === 'string' && value.length <= 2048 && !value.includes('\0') ? undefined : nls.localize('settings.mediaSource', 'Choose an available source.');
}

function relayInfos(value: unknown): string | undefined {
	return Array.isArray(value) && value.length <= 8 && value.every((relay: unknown) => typeof relay === 'object' && relay !== null && 'url' in relay && typeof relay.url === 'string'
		&& 'authenticated' in relay && typeof relay.authenticated === 'boolean' && ('replacementToken' in relay ? typeof relay.replacementToken === 'string' && Object.keys(relay).length === 3 : Object.keys(relay).length === 2))
		? undefined : nls.localize('connection.relayCount', 'Enter between one and eight custom relays.');
}

registerSettings('user', {
	id: 'application',
	title: nls.localize('home.general', 'General'),
	properties: {
		'application.acceptConnections': {
			type: 'boolean',
			default: true,
			title: nls.localize('application.accept', 'Accept connections from paired devices'),
			description: nls.localize('application.acceptHelp', 'When off, paired devices cannot connect and anyone connected is disconnected, until you turn it back on.'),
		},
		'application.runInBackground': {
			type: 'boolean',
			default: true,
			title: nls.localize('application.background', 'Keep running when the window is closed'),
			description: nls.localize('application.backgroundHelp', 'Paired devices can still connect to this computer.'),
			restrictedReason: nls.localize('application.backgroundUnavailable', 'Available once VniLink shows an icon in the system tray.'),
		},
	},
});

registerSettings('system', {
	id: 'application.login',
	title: nls.localize('home.general', 'General'),
	properties: {
		'application.openAtLogin': {
			type: 'boolean',
			default: false,
			title: nls.localize('application.login', 'Start when you sign in'),
			description: nls.localize('application.loginHelp', 'VniLink starts in the background so paired devices can connect.'),
			restrictedReason: nls.localize('application.loginUnavailable', 'Available in the installed app.'),
		},
	},
});

registerSettings('user', {
	id: 'appearance',
	title: nls.localize('home.appearance', 'Appearance'),
	properties: {
		'appearance.theme': {
			type: 'string',
			enum: ['system', 'light', 'dark'],
			enumItemLabels: [nls.localize('appearance.system', 'System'), nls.localize('appearance.light', 'Light'), nls.localize('appearance.dark', 'Dark')],
			default: 'system',
			title: nls.localize('appearance.theme', 'Color theme'),
			description: nls.localize('appearance.themeDescription', 'Choose a look, or let it change with your system.'),
		},
	},
});

registerSettings('user', {
	id: 'desktop',
	title: nls.localize('home.desktop', 'Remote Desktop'),
	properties: {
		'desktop.quality': {
			type: 'object',
			default: { max_bitrate_kbps: 2000, max_frame_rate: 30, max_width: 1920, max_height: 1080 },
			title: nls.localize('desktop.defaultQuality', 'Default stream quality'),
			description: nls.localize('desktop.defaultsHelp', 'Default limits for new outgoing desktop sessions. The host may negotiate lower values. Existing sessions keep their current settings.'),
			validate: value => isQualityLimits(value) ? undefined : nls.localize('desktop.invalidQuality', 'Enter limits up to 50,000 kbps, 60 fps and 7680 × 7680 px.'),
		},
		'desktop.codec': {
			type: 'string',
			default: 'auto',
			title: nls.localize('desktop.codecChoice', 'Preferred codec'),
			description: nls.localize('desktop.codecHelp', 'Auto chooses a codec supported by both computers. A specific choice requires support on both sides. Changes apply to new desktop sessions.'),
			validate: value => isCodecPreference(value) ? undefined : nls.localize('settings.choice', 'Choose one of the available options.'),
		},
	},
});

registerSettings('user', {
	id: 'input',
	title: nls.localize('home.input', 'Keyboard & Mouse'),
	properties: {
		'input.keyboardMode': {
			type: 'string',
			enum: ['Character', 'Map', 'Translate'],
			enumItemLabels: ['Characters', 'Physical keys', 'Translate to host layout'],
			default: 'Character',
			title: nls.localize('input.mapping', 'Mapping'),
			description: nls.localize('input.mappingHelp', 'Characters sends the text you type. Physical keys uses the remote keyboard layout. Translate sends matching key combinations and requires a Linux X11 host.'),
		},
		'input.swapModifiers': {
			type: 'boolean',
			default: false,
			title: nls.localize('input.swap', 'Swap Control and Command'),
			description: nls.localize('input.swapHelp', 'Use familiar shortcuts when moving between Mac and Windows or Linux.'),
		},
		'input.pointerSensitivity': {
			type: 'integer',
			minimum: 1,
			maximum: 400,
			default: 100,
			unit: '%',
			title: nls.localize('input.sensitivity', 'Relative pointer sensitivity'),
			description: nls.localize('input.relativeHelp', '100% keeps mouse movement unchanged. Absolute pointer positioning is unaffected.'),
		},
		'input.reverseWheel': {
			type: 'boolean',
			default: false,
			title: nls.localize('input.wheel', 'Reverse scrolling'),
			description: nls.localize('input.wheelHelp', 'Reverse both horizontal and vertical scrolling on the remote computer.'),
		},
		'input.swapMouseButtons': {
			type: 'boolean',
			default: false,
			title: nls.localize('input.buttons', 'Swap primary and secondary buttons'),
			description: nls.localize('input.buttonsHelp', 'Use the right mouse button for primary clicks.'),
		},
	},
});

registerSettings('user', {
	id: 'media',
	title: nls.localize('home.media', 'Audio & Camera'),
	properties: {
		'media.camera': {
			type: 'string',
			default: 'vnilink:auto-camera',
			title: nls.localize('media.cameras', 'Cameras'),
			description: nls.localize('media.camerasHelp', 'Choose the camera this computer shares with permitted devices.'),
			validate: mediaSource,
		},
		'media.microphone': {
			type: 'string',
			default: 'vnilink:default-microphone',
			title: nls.localize('media.microphones', 'Microphones'),
			description: nls.localize('media.microphonesHelp', 'Choose the microphone used when a permitted device requests it.'),
			validate: mediaSource,
		},
		'media.systemAudio': {
			type: 'string',
			default: 'vnilink:default-system-audio',
			title: nls.localize('media.system', 'System audio'),
			description: nls.localize('media.systemHelp', 'Share this computer’s sound without changing its local speakers.'),
			validate: mediaSource,
		},
	},
});

registerSettings('user', {
	id: 'recording',
	title: nls.localize('home.recording', 'Recording'),
	properties: {
		'recording.directory': {
			type: 'string',
			default: '',
			title: nls.localize('recording.destination', 'Save recordings'),
			description: nls.localize('recording.destinationHelp', 'Each recording gets its own file in this folder.'),
			validate: value => value === '' || (typeof value === 'string' && isAbsolutePath(value)) ? undefined : nls.localize('recording.invalidFolder', 'Choose a folder on this computer.'),
		},
		'recording.maxSegmentSize': {
			type: 'integer',
			minimum: 1,
			maximum: 1048576,
			default: 1024,
			unit: 'MiB',
			title: nls.localize('recording.size', 'Maximum segment size'),
		},
		'recording.maxSegmentDuration': {
			type: 'integer',
			minimum: 1,
			maximum: 1440,
			default: 60,
			unit: nls.localize('recording.minutes', 'min'),
			title: nls.localize('recording.duration', 'Maximum segment duration'),
		},
		'recording.maxSegments': {
			type: 'integer',
			minimum: 1,
			maximum: 65535,
			default: 10,
			unit: nls.localize('recording.segmentUnit', 'segments'),
			title: nls.localize('recording.segments', 'Keep up to'),
		},
		'recording.maxTotalSize': {
			type: 'integer',
			minimum: 1,
			maximum: 1048576,
			default: 10240,
			unit: 'MiB',
			title: nls.localize('recording.storage', 'Maximum combined size'),
		},
	},
});

registerSettings('user', {
	id: 'files',
	title: nls.localize('home.files', 'Files'),
	properties: {
		'files.roots': {
			type: 'array',
			items: { type: 'string' },
			default: [],
			title: nls.localize('files.shared', 'Folder restrictions'),
			description: nls.localize('files.help', 'By default, permitted devices can browse the filesystem within this app’s operating-system access. Add folders only to restrict browsing. Devices still need Files permission.'),
			validate: value => isFileRoots(value) ? undefined : nls.localize('files.invalid', 'Use unique, full folder paths. Remove empty rows before saving.'),
		},
	},
});

const keepAwake = (scope: 'incoming' | 'outgoing'): IStringDictionary<ISettingSchema> => ({
	[`power.${scope}.keepSystemAwake`]: {
		type: 'boolean',
		default: false,
		title: nls.localize('power.system', 'Keep the computer awake'),
		description: nls.localize('power.systemHelp', 'Prevent system sleep while the session is being used.'),
	},
	[`power.${scope}.keepDisplayAwake`]: {
		type: 'boolean',
		default: false,
		title: nls.localize('power.display', 'Keep the display awake'),
		description: nls.localize('power.displayHelp', 'Prevent the display from sleeping during the session.'),
	},
	[`power.${scope}.idleTimeout`]: {
		type: 'number',
		minimum: 0,
		maximum: 1440,
		default: 0,
		unit: nls.localize('power.minutes', 'min'),
		title: nls.localize('power.idle', 'End idle desktop sessions after'),
		validate: minutes,
	},
});

registerSettings('power', {
	id: 'power',
	title: nls.localize('home.power', 'Power'),
	properties: {
		...keepAwake('incoming'),
		...keepAwake('outgoing'),
		'power.lockAfterControl': {
			type: 'boolean',
			default: false,
			title: nls.localize('power.lock', 'Lock this computer'),
			description: nls.localize('power.lockHelp', 'Ask the operating system to lock when remote control ends.'),
		},
	},
});

registerSettings('terminal', {
	id: 'terminal',
	title: nls.localize('home.terminal', 'Terminal'),
	properties: {
		'terminal.maxSessions': {
			type: 'integer',
			minimum: 1,
			maximum: 16,
			default: 16,
			title: nls.localize('terminal.total', 'Maximum terminals'),
			description: nls.localize('terminal.totalHelp', 'Total across all paired devices.'),
		},
		'terminal.maxSessionsPerDevice': {
			type: 'integer',
			minimum: 1,
			maximum: 16,
			default: 16,
			title: nls.localize('terminal.perDevice', 'Maximum per device'),
			description: nls.localize('terminal.perDeviceHelp', 'Cannot exceed the total limit.'),
		},
		'terminal.persistence': {
			type: 'boolean',
			default: true,
			title: nls.localize('terminal.persistence', 'Keep detached terminals running'),
			description: nls.localize('terminal.persistenceHelp', 'Allow a device to leave a terminal running and reconnect to it later.'),
		},
		'terminal.detachedTimeout': {
			type: 'integer',
			minimum: 1,
			maximum: 86400,
			default: 300,
			unit: 's',
			title: nls.localize('terminal.timeout', 'Detached lifetime'),
			description: nls.localize('terminal.timeoutHelp', 'Stop a detached terminal after this many seconds. Used when persistence is allowed.'),
		},
		'terminal.historySize': {
			type: 'number',
			minimum: 16,
			maximum: 4096,
			default: 1024,
			unit: 'KiB',
			title: nls.localize('terminal.historySize', 'History limit'),
			description: nls.localize('terminal.historyHelp', 'Maximum output retained per terminal, from 16 KiB to 4 MiB.'),
			validate: kibibytes,
		},
		'terminal.replaySize': {
			type: 'number',
			minimum: 0,
			maximum: 4096,
			default: 8,
			unit: 'KiB',
			title: nls.localize('terminal.replay', 'Replay on reconnect'),
			description: nls.localize('terminal.replayHelp', 'Output replayed when a device reconnects. Cannot exceed the history limit; zero disables replay.'),
			validate: kibibytes,
		},
	},
});

/** Limits that span settings: a device's share and the replay window stay within their totals. */
export function validateTerminalLimits(values: Readonly<Record<string, unknown>>): string | undefined {
	const total = values['terminal.maxSessions'];
	const perDevice = values['terminal.maxSessionsPerDevice'];
	const history = values['terminal.historySize'];
	const replay = values['terminal.replaySize'];
	return typeof total === 'number' && typeof perDevice === 'number' && typeof history === 'number' && typeof replay === 'number' && perDevice <= total && replay <= history
		? undefined
		: nls.localize('terminal.invalid', 'Check the limits: 1–16 terminals, per-device limit no higher than the total, history from 16 KiB to 4 MiB, replay no larger than history, and lifetime from 1 to 86,400 seconds.');
}

const sharing = (key: string, title: string, description: string): IStringDictionary<ISettingSchema> => ({ [`sharing.${key}`]: { type: 'boolean', default: false, title, description } });

registerSettings('sharing', {
	id: 'sharing',
	title: nls.localize('sharing.title', 'Sharing with paired computers'),
	properties: {
		...sharing('screen', nls.localize('sharing.screen', 'Screen viewing and remote control'), nls.localize('sharing.screenHelp', 'Allow paired computers to view this screen and use the keyboard and mouse.')),
		...sharing('files', nls.localize('sharing.files', 'Files'), nls.localize('sharing.filesHelp', 'Allow file browsing and transfers on this computer.')),
		...sharing('terminal', nls.localize('sharing.terminal', 'Terminal'), nls.localize('sharing.terminalHelp', 'Allow shell sessions on this computer.')),
		...sharing('camera', nls.localize('sharing.camera', 'Camera'), nls.localize('sharing.cameraHelp', 'Allow access to the camera sources you share.')),
		...sharing('microphone', nls.localize('sharing.microphone', 'Microphone'), nls.localize('sharing.microphoneHelp', 'Allow listening to shared microphone input.')),
		...sharing('systemAudio', nls.localize('sharing.audio', 'System audio'), nls.localize('sharing.audioHelp', 'Allow listening to audio played by this computer.')),
		...sharing('clipboard', nls.localize('sharing.clipboard', 'Clipboard'), nls.localize('sharing.clipboardHelp', 'Allow clipboard exchange during desktop sessions.')),
		...sharing('recording', nls.localize('sharing.recording', 'Recording'), nls.localize('sharing.recordingHelp', 'Allow recording through VniLink during permitted desktop sessions.')),
		...sharing('systemActions', nls.localize('sharing.actions', 'System actions'), nls.localize('sharing.actionsHelp', 'Allow the supported actions enabled under Remote action rules.')),
	},
});

const action = (key: string, title: string, description: string, allowed = false): IStringDictionary<ISettingSchema> => ({ [`security.${key}`]: { type: 'boolean', default: allowed, title, description } });

registerSettings('security', {
	id: 'security',
	title: nls.localize('security.local', 'Remote action rules'),
	properties: {
		...action('allowLock', nls.localize('security.lock', 'Allow remote lock'), nls.localize('security.lockHelp', 'Let permitted devices lock this computer.'), true),
		...action('allowRestart', nls.localize('security.restart', 'Allow remote restart'), nls.localize('security.restartHelp', 'Let permitted devices restart this computer.')),
		...action('allowShutdown', nls.localize('security.shutdown', 'Allow remote shutdown'), nls.localize('security.shutdownHelp', 'Let permitted devices shut down this computer.')),
		...action('allowPrivacy', nls.localize('security.privacy', 'Allow privacy mode'), nls.localize('security.privacyHelp', 'Let permitted devices request privacy mode during remote sessions.')),
	},
});

registerSettings('relays', {
	id: 'connection',
	title: nls.localize('home.connection', 'Connection'),
	properties: {
		'connection.relays': {
			type: 'array',
			default: [],
			title: nls.localize('connection.servers', 'Relay servers'),
			description: nls.localize('connection.customDescription', 'Up to eight servers. Use HTTPS, or HTTP for a loopback address.'),
			validate: relayInfos,
		},
	},
});

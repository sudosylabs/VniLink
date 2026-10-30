// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import type { IStringDictionary } from '../../../base/common/collections.js';
import { Registry } from '../../registry/common/platform.js';
import { isMacintosh } from '../../../base/common/platform.js';
import { isCodecPreference, isQualityLimits, MAX_LOG_FILTER_BYTES, shellId } from '../../engine/common/engine.js';
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

/** A media source id; the empty id turns the source off. */
function mediaSource(value: unknown): string | undefined {
	return typeof value === 'string' && value.length <= 2048 && !value.includes('\0') ? undefined : nls.localize('settings.mediaSource', 'Choose an available source.');
}

/** A detected shell's id; the empty id offers the automatic shell. */
function shellChoice(value: unknown): string | undefined {
	return value === '' || shellId(value) ? undefined : nls.localize('terminal.shellChoice', 'Choose a shell installed on this computer.');
}

/** Recording sizes are entered in GiB to one decimal place. */
function tenthsOfGibibytes(value: unknown): string | undefined {
	return typeof value === 'number' && Math.abs(value * 10 - Math.round(value * 10)) < 1e-9 ? undefined : nls.localize('recording.tenths', 'Enter a size to one decimal place, for example 1.5.');
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
			title: nls.localize('application.accept', 'Accept connections from paired computers'),
			description: isMacintosh
				? nls.localize('application.acceptHelpMac', 'When off, connected computers are disconnected. The menu bar icon has the same switch.')
				: nls.localize('application.acceptHelp', 'When off, connected computers are disconnected. The tray icon has the same switch.'),
		},
		'application.runInBackground': {
			type: 'boolean',
			default: true,
			title: nls.localize('application.background', 'Keep running when the window is closed'),
			restrictedReason: nls.localize('application.backgroundUnavailable', 'Needs a panel that shows tray icons. On GNOME, install the AppIndicator extension.'),
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
			title: nls.localize('appearance.theme', 'Theme'),
		},
	},
});

registerSettings('user', {
	id: 'desktop',
	title: nls.localize('home.desktop', 'Remote Desktop'),
	properties: {
		'desktop.quality': {
			type: 'object',
			default: { max_bitrate_kbps: 50000, max_frame_rate: 30, max_width: 1920, max_height: 1080 },
			title: nls.localize('desktop.quality', 'Stream limits'),
			description: nls.localize('desktop.qualityHelp', 'The most bandwidth, frame rate and picture size a stream may use. A stream uses what its picture and the connection allow, up to these limits.'),
			validate: value => isQualityLimits(value) ? undefined : nls.localize('desktop.invalidQuality', 'Enter limits up to 50,000 kbps, 60 fps and 7680 × 7680 px.'),
		},
		'desktop.pictureSize': {
			type: 'string',
			enum: ['fit', 'stretch', 'original'],
			enumItemLabels: [nls.localize('desktop.pictureFit', 'Fit'), nls.localize('desktop.pictureStretch', 'Stretch'), nls.localize('desktop.pictureOriginal', 'Original size')],
			enumDescriptions: [
				nls.localize('desktop.pictureFitHelp', 'Each pixel of the other screen on one pixel of this one, made smaller when the tab can’t hold it.'),
				nls.localize('desktop.pictureStretchHelp', 'Made larger or smaller to fill the tab.'),
				nls.localize('desktop.pictureOriginalHelp', 'Each pixel of the other screen on one pixel of this one; scroll to see what the tab can’t hold.'),
			],
			default: 'fit',
			title: nls.localize('desktop.pictureSize', 'Picture size'),
		},
		'desktop.codec': {
			type: 'string',
			default: 'auto',
			title: nls.localize('desktop.codecChoice', 'Codec'),
			description: nls.localize('desktop.codecHelp', 'Automatic picks one both computers support.'),
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
			enumItemLabels: [nls.localize('input.characters', 'Characters'), nls.localize('input.physicalKeys', 'Physical keys'), nls.localize('input.translate', 'Translate shortcuts')],
			enumDescriptions: [
				nls.localize('input.charactersHelp', 'Sends the text you type, whatever the other keyboard layout.'),
				nls.localize('input.physicalKeysHelp', 'Sends the keys you press; the other computer’s layout decides the letter.'),
				nls.localize('input.translateHelp', 'Sends matching shortcuts. Needs a Linux computer running X11.'),
			],
			default: 'Character',
			title: nls.localize('input.mapping', 'Typing'),
		},
		'input.swapModifiers': {
			type: 'string',
			enum: ['automatic', 'always', 'never'],
			enumItemLabels: [nls.localize('input.swapAutomatic', 'Between a Mac and a PC'), nls.localize('input.swapAlways', 'Always'), nls.localize('input.swapNever', 'Never')],
			enumDescriptions: [
				nls.localize('input.swapAutomaticHelp', 'Control on a PC is Command on a Mac, and the reverse, so shortcuts such as copy and undo work.'),
				nls.localize('input.swapAlwaysHelp', 'Exchanges them with every computer.'),
				nls.localize('input.swapNeverHelp', 'Each key reaches the other computer as itself.'),
			],
			default: 'automatic',
			title: nls.localize('input.swap', 'Swap Control and Command'),
			description: nls.localize('input.swapHelp', 'A remote desktop’s menu changes it for one computer.'),
		},
		'input.pointerSensitivity': {
			type: 'integer',
			minimum: 1,
			maximum: 400,
			default: 100,
			unit: '%',
			title: nls.localize('input.sensitivity', 'Pointer speed'),
		},
		'input.reverseWheel': {
			type: 'boolean',
			default: false,
			title: nls.localize('input.wheel', 'Reverse scrolling'),
		},
		'input.swapMouseButtons': {
			type: 'boolean',
			default: false,
			title: nls.localize('input.buttons', 'Swap left and right buttons'),
		},
	},
});

registerSettings('user', {
	id: 'media',
	title: nls.localize('home.media', 'Audio & Camera'),
	properties: {
		'media.camera': {
			type: 'string',
			default: 'vnilink:all-cameras',
			title: nls.localize('media.camera', 'Camera'),
			description: nls.localize('media.cameraHelp', 'The cameras paired computers can view.'),
			validate: mediaSource,
		},
		'media.microphone': {
			type: 'string',
			default: 'vnilink:default-microphone',
			title: nls.localize('media.microphone', 'Microphone'),
			validate: mediaSource,
		},
		'media.systemAudio': {
			type: 'string',
			default: 'vnilink:default-system-audio',
			title: nls.localize('media.system', 'System audio'),
			description: nls.localize('media.systemHelp', 'Your own speakers keep playing.'),
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
			title: nls.localize('recording.folder', 'Folder'),
			validate: value => value === '' || (typeof value === 'string' && isAbsolutePath(value)) ? undefined : nls.localize('recording.invalidFolder', 'Choose a folder on this computer.'),
		},
		'recording.maxSegmentSize': {
			type: 'number',
			minimum: 0.1,
			maximum: 1024,
			default: 1,
			unit: 'GiB',
			title: nls.localize('recording.size', 'File size'),
			validate: tenthsOfGibibytes,
		},
		'recording.maxSegmentDuration': {
			type: 'integer',
			minimum: 1,
			maximum: 1440,
			default: 60,
			unit: nls.localize('recording.minutes', 'min'),
			title: nls.localize('recording.duration', 'File length'),
		},
		'recording.maxSegments': {
			type: 'integer',
			minimum: 1,
			maximum: 65535,
			default: 10,
			title: nls.localize('recording.segments', 'Files per recording'),
		},
		'recording.maxTotalSize': {
			type: 'number',
			minimum: 0.1,
			maximum: 1024,
			default: 10,
			unit: 'GiB',
			title: nls.localize('recording.storage', 'Size per recording'),
			validate: tenthsOfGibibytes,
		},
	},
});

const keepAwake = (scope: 'incoming' | 'outgoing'): IStringDictionary<ISettingSchema> => ({
	[`power.${scope}.keepSystemAwake`]: {
		type: 'boolean',
		default: false,
		title: nls.localize('power.system', 'Keep this computer awake'),
	},
	[`power.${scope}.keepDisplayAwake`]: {
		type: 'boolean',
		default: false,
		title: nls.localize('power.display', 'Keep the display on'),
	},
	[`power.${scope}.idleTimeout`]: {
		type: 'number',
		minimum: 0,
		maximum: 1440,
		default: 0,
		unit: nls.localize('power.minutes', 'min'),
		title: nls.localize('power.idle', 'End idle sessions after'),
		description: nls.localize('power.idleHelp', '0 never ends them.'),
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
		},
	},
});

registerSettings('user', {
	id: 'terminalShell',
	title: nls.localize('home.terminal', 'Terminal'),
	properties: {
		'terminal.shell': {
			type: 'string',
			default: '',
			title: nls.localize('terminal.shell', 'Shell'),
			description: nls.localize('terminal.shellHelp', 'Paired computers can’t choose another.'),
			validate: shellChoice,
		},
	},
});

/** A filter the engine reads; it checks the syntax itself when the setting is applied. */
function logFilterText(value: unknown): string | undefined {
	return typeof value === 'string' && new TextEncoder().encode(value).length <= MAX_LOG_FILTER_BYTES ? undefined : nls.localize('diagnostics.filterLength', 'Enter a shorter filter.');
}

registerSettings('user', {
	id: 'diagnostics',
	title: nls.localize('diagnostics.title', 'Diagnostics'),
	properties: {
		'diagnostics.applicationLog': {
			type: 'string',
			enum: ['off', 'error', 'warn', 'info', 'debug', 'trace'],
			enumItemLabels: [
				nls.localize('diagnostics.off', 'Off'),
				nls.localize('diagnostics.error', 'Errors'),
				nls.localize('diagnostics.warn', 'Warnings'),
				nls.localize('diagnostics.info', 'Information'),
				nls.localize('diagnostics.debug', 'Debug'),
				nls.localize('diagnostics.trace', 'Everything'),
			],
			default: 'info',
			title: nls.localize('diagnostics.applicationLog', 'Application log'),
			description: nls.localize('diagnostics.applicationLogHelp', 'What VniLink records about its windows and settings.'),
		},
		'diagnostics.engineLog': {
			type: 'string',
			enum: ['off', 'error', 'warn', 'info', 'debug', 'trace'],
			enumItemLabels: [
				nls.localize('diagnostics.off', 'Off'),
				nls.localize('diagnostics.error', 'Errors'),
				nls.localize('diagnostics.warn', 'Warnings'),
				nls.localize('diagnostics.info', 'Information'),
				nls.localize('diagnostics.debug', 'Debug'),
				nls.localize('diagnostics.trace', 'Everything'),
			],
			default: 'info',
			title: nls.localize('diagnostics.engineLog', 'Engine log'),
			description: nls.localize('diagnostics.engineLogHelp', 'What VniLink records in its engine log. Higher levels help explain a problem and take more space.'),
		},
		'diagnostics.engineLogFilter': {
			type: 'string',
			default: '',
			title: nls.localize('diagnostics.engineLogFilter', 'Engine log filter'),
			description: nls.localize('diagnostics.engineLogFilterHelp', 'Levels per area, such as vnilink::files=debug; replaces the engine log level when set. The VNILINK_LOG environment variable replaces both.'),
			validate: logFilterText,
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
			title: nls.localize('terminal.total', 'Terminals at once'),
			description: nls.localize('terminal.totalHelp', 'Across all paired computers.'),
		},
		'terminal.maxSessionsPerDevice': {
			type: 'integer',
			minimum: 1,
			maximum: 16,
			default: 16,
			title: nls.localize('terminal.perDevice', 'Per computer'),
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
	if (typeof total !== 'number' || typeof perDevice !== 'number' || perDevice > total) {
		return nls.localize('terminal.perDeviceInvalid', 'Per computer can’t be more than Terminals at once.');
	}
	return typeof history === 'number' && typeof replay === 'number' && replay <= history
		? undefined
		: nls.localize('terminal.replayInvalid', 'Replay can’t be larger than the history limit.');
}

const sharing = (key: string, title: string, description?: string): IStringDictionary<ISettingSchema> => ({ [`sharing.${key}`]: { type: 'boolean', default: false, title, description } });

registerSettings('sharing', {
	id: 'sharing',
	title: nls.localize('sharing.title', 'Sharing with paired computers'),
	properties: {
		...sharing('screen', nls.localize('sharing.screen', 'Screen viewing and remote control')),
		...sharing('files', nls.localize('sharing.files', 'Files')),
		...sharing('terminal', nls.localize('sharing.terminal', 'Terminal')),
		...sharing('camera', nls.localize('sharing.camera', 'Camera')),
		...sharing('microphone', nls.localize('sharing.microphone', 'Microphone')),
		...sharing('systemAudio', nls.localize('sharing.audio', 'System audio'), nls.localize('sharing.audioHelp', 'Turns on with screen viewing. Turn it off to share the picture without sound.')),
		...sharing('clipboard', nls.localize('sharing.clipboard', 'Clipboard'), nls.localize('sharing.clipboardHelp', 'During remote desktop sessions.')),
		...sharing('recording', nls.localize('sharing.recording', 'Record desktop sessions')),
		...sharing('systemActions', nls.localize('sharing.actions', 'System actions'), nls.localize('sharing.actionsHelp', 'Only the actions turned on under Remote actions.')),
	},
});

const action = (key: string, title: string, description?: string, allowed = false): IStringDictionary<ISettingSchema> => ({ [`security.${key}`]: { type: 'boolean', default: allowed, title, description } });

registerSettings('security', {
	id: 'security',
	title: nls.localize('security.local', 'Remote actions'),
	properties: {
		...action('allowLock', nls.localize('security.lock', 'Lock this computer'), undefined, true),
		...action('allowRestart', nls.localize('security.restart', 'Restart')),
		...action('allowShutdown', nls.localize('security.shutdown', 'Shut down')),
		...action('allowPrivacy', nls.localize('security.privacy', 'Privacy mode'), nls.localize('security.privacyHelp', 'Blanks this display for people nearby during a remote session.')),
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

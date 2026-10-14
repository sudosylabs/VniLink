import { isCodecPreference, type CodecPreference, isQualityLimits, type QualityLimits, type PointerSettings } from '../../engine/common/engine.js';
import type { Event } from '../../../base/common/event.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';

export interface InputDefaults { readonly mode: 'Character' | 'Map' | 'Translate'; readonly swap: boolean; readonly pointer: Readonly<PointerSettings>; }

export function isInputDefaults(value: unknown): value is InputDefaults {
	if (typeof value !== 'object' || value === null || !('mode' in value) || !('swap' in value) || !('pointer' in value) || Object.keys(value).length !== 3) {
		return false;
	}
	const p = value.pointer;
	return typeof value.mode === 'string' && ['Character', 'Map', 'Translate'].includes(value.mode) && typeof value.swap === 'boolean' && typeof p === 'object' && p !== null && 'sensitivity_percent' in p && typeof p.sensitivity_percent === 'number' && Number.isInteger(p.sensitivity_percent) && p.sensitivity_percent >= 1 && p.sensitivity_percent <= 400 && 'reverse_wheel' in p && typeof p.reverse_wheel === 'boolean' && 'swap_buttons' in p && typeof p.swap_buttons === 'boolean' && Object.keys(p).length === 3;
}

export function isFileRoots(value: unknown): value is readonly string[] {
	return Array.isArray(value) && value.length <= 16 && new Set(value).size === value.length && value.every((path: unknown) => typeof path === 'string' && path.length > 0 && path.length <= 4096 && !path.includes('\0') && (path.startsWith('/') || /^[a-z]:[\\/]/i.test(path) || /^\\\\[^\\]+\\[^\\]+/.test(path)));
}

export interface ConfigurationValues {
	readonly 'input.defaults': InputDefaults;
	readonly 'files.roots': readonly string[];
	readonly 'desktop.codec': CodecPreference;
	readonly 'desktop.quality': Readonly<QualityLimits>;
	readonly 'appearance.theme': 'system' | 'light' | 'dark';
	readonly 'appearance.nativeBackground': boolean;
}

export type ConfigurationUpdate = {
	[K in keyof ConfigurationValues]: { readonly key: K; readonly value: ConfigurationValues[K] }
}[keyof ConfigurationValues];

export interface ConfigurationSnapshot {
	readonly revision: number;
	readonly values: ConfigurationValues;
	readonly error: string | null;
}

export const defaultConfiguration: ConfigurationValues = Object.freeze({
	'input.defaults': Object.freeze({ mode: 'Character', swap: false, pointer: Object.freeze({ sensitivity_percent: 100, reverse_wheel: false, swap_buttons: false }) }),
	'files.roots': Object.freeze([]),
	'desktop.codec': 'auto',
	'desktop.quality': Object.freeze({ max_bitrate_kbps: 2000, max_frame_rate: 30, max_width: 1920, max_height: 1080 }),
	'appearance.theme': 'system',
	'appearance.nativeBackground': true,
});

export function isConfigurationValues(value: unknown): value is ConfigurationValues {
	return typeof value === 'object' && value !== null
		&& 'appearance.theme' in value && (value['appearance.theme'] === 'system' || value['appearance.theme'] === 'light' || value['appearance.theme'] === 'dark')
		&& 'appearance.nativeBackground' in value && typeof value['appearance.nativeBackground'] === 'boolean'
		&& 'desktop.codec' in value && isCodecPreference(value['desktop.codec'])
		&& 'desktop.quality' in value && isQualityLimits(value['desktop.quality'])
		&& 'input.defaults' in value && isInputDefaults(value['input.defaults'])
		&& 'files.roots' in value && isFileRoots(value['files.roots'])
		&& Object.keys(value).length === 6;
}

export function isConfigurationUpdate(value: unknown): value is ConfigurationUpdate {
	return typeof value === 'object' && value !== null && 'key' in value && 'value' in value
		&& Object.keys(value).length === 2
		&& ((value.key === 'appearance.theme' && (value.value === 'system' || value.value === 'light' || value.value === 'dark'))
			|| (value.key === 'appearance.nativeBackground' && typeof value.value === 'boolean')
			|| (value.key === 'input.defaults' && isInputDefaults(value.value))
			|| (value.key === 'files.roots' && isFileRoots(value.value))
			|| (value.key === 'desktop.codec' && isCodecPreference(value.value))
			|| (value.key === 'desktop.quality' && isQualityLimits(value.value)));
}

export function isConfigurationSnapshot(value: unknown): value is ConfigurationSnapshot {
	return typeof value === 'object' && value !== null
		&& 'revision' in value && typeof value.revision === 'number' && Number.isSafeInteger(value.revision) && value.revision >= 0
		&& 'values' in value && isConfigurationValues(value.values)
		&& 'error' in value && (value.error === null || typeof value.error === 'string') && Object.keys(value).length === 3;
}

export const IConfigurationService = createDecorator<IConfigurationService>('configurationService');

export interface IConfigurationService {
	readonly _serviceBrand: undefined;
	readonly snapshot: ConfigurationSnapshot;
	readonly onDidChangeConfiguration: Event<ConfigurationSnapshot>;
	initialize(): Promise<void>;
	updateValue(change: ConfigurationUpdate): Promise<void>;
}

export function freezeConfiguration(values: ConfigurationValues): ConfigurationValues {
	return Object.freeze({ ...values, 'desktop.quality': Object.freeze({ ...values['desktop.quality'] }), 'input.defaults': Object.freeze({ ...values['input.defaults'], pointer: Object.freeze({ ...values['input.defaults'].pointer }) }), 'files.roots': Object.freeze([...values['files.roots']]) });
}

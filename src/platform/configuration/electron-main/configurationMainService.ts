import { Emitter, Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';
import { isCodecPreference, isQualityLimits, type EngineCommand } from '../../engine/common/engine.js';
import { IEngineMainService } from '../../engine/electron-main/engineMainService.js';
import type { EngineMainService } from '../../engine/electron-main/engineMainService.js';
import { UserSettings } from '../node/userSettings.js';
import { isFileRoots, settingSource, validateSetting } from '../common/configurationSchema.js';
import type { ConfigurationSnapshot } from '../common/configurationSnapshot.js';
import { EngineSettings } from './engineSettings.js';

export const IConfigurationMainService = createDecorator<ConfigurationMainService>('configurationMainService');

type Values = Readonly<Record<string, unknown>>;

const keyboardModes = ['Character', 'Map', 'Translate'] as const;

/** Engine commands that apply one group of user settings. Values were validated against the schema. */
function engineCommands(group: string, values: Values): EngineCommand[] {
	switch (group) {
		case 'desktop.quality': {
			const quality = values['desktop.quality'];
			return isQualityLimits(quality) ? [{ type: 'setDesktopQualityDefaults', quality }] : [];
		}
		case 'desktop.codec': {
			const codec = values['desktop.codec'];
			return isCodecPreference(codec) ? [{ type: 'setDesktopCodecDefault', codec }] : [];
		}
		case 'input': {
			const mode = keyboardModes.find(candidate => candidate === values['input.keyboardMode']) ?? 'Character';
			const pointer = { sensitivity_percent: Number(values['input.pointerSensitivity']), reverse_wheel: values['input.reverseWheel'] === true, swap_buttons: values['input.swapMouseButtons'] === true };
			return [{ type: 'setInputDefaults', mode, swap: values['input.swapModifiers'] === true, pointer }];
		}
		case 'media.camera':
			return [{ type: 'setNativeCameraSources', ids: sourceIds(values['media.camera']) }];
		case 'media.audio':
			return [{ type: 'setNativeAudioSources', microphones: sourceIds(values['media.microphone']), system: sourceIds(values['media.systemAudio']) }];
		case 'files.roots': {
			const paths = fileRoots(values);
			return [paths.length ? { type: 'setFileRoots', paths } : { type: 'enableFilesystemAccess' }];
		}
	}
	return [];
}

function fileRoots(values: Values): string[] {
	const paths = values['files.roots'];
	return isFileRoots(paths) ? [...paths] : [];
}

/** The engine group a user setting belongs to; settings without one stay in this process. */
function engineGroup(key: string): string | undefined {
	if (key === 'desktop.quality' || key === 'desktop.codec' || key === 'files.roots' || key === 'media.camera') {
		return key;
	}
	if (key === 'media.microphone' || key === 'media.systemAudio') {
		return 'media.audio';
	}
	return key.startsWith('input.') ? 'input' : undefined;
}

function sourceIds(value: unknown): string[] {
	return typeof value === 'string' && value ? [value] : [];
}

/**
 * The single owner of settings. User settings are applied to the engine before
 * they are committed and rolled back from the engine when the commit fails, so
 * disk and runtime never disagree; engine settings are written through to Rust.
 */
export class ConfigurationMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly user: UserSettings;
	private readonly engineSettings: EngineSettings;
	private readonly changed = this._register(new Emitter<ConfigurationSnapshot>());
	public readonly onDidChangeConfiguration: Event<ConfigurationSnapshot> = this.changed.event;
	private current: ConfigurationSnapshot | undefined;
	private revision = 0;
	private pending: Promise<unknown> = Promise.resolve();

	constructor(
		path: string,
		@IEngineMainService private readonly engine: EngineMainService,
		@ILogService private readonly logService: ILogService,
	) {
		super();
		this.user = this._register(new UserSettings(path));
		this.engineSettings = this._register(new EngineSettings(engine));
		this._register(Event.any(this.user.onDidChange, this.engineSettings.onDidChange)(() => {
			this.current = undefined;
			this.changed.fire(this.snapshot);
		}));
	}

	public get snapshot(): ConfigurationSnapshot {
		this.current ??= {
			revision: this.revision++,
			values: { ...this.user.values, ...this.engineSettings.values },
			errors: { ...this.engineSettings.sourceErrors, ...(this.user.error ? { user: this.user.error } : {}) },
			restricted: this.engineSettings.restricted,
		};
		return this.current;
	}

	/** The current value of a user setting. */
	public getValue(key: string): unknown {
		return this.user.values[key];
	}

	public initialize(): Promise<void> {
		return this.user.initialize();
	}

	public flush(): Promise<void> {
		return this.user.flush();
	}

	/** Reads the engine's settings again, for changes made outside this app. */
	public reload(): Promise<void> {
		return this.engineSettings.refresh();
	}

	public updateValue(key: string, value: unknown): Promise<ConfigurationSnapshot> {
		const invalid = validateSetting(key, value);
		if (invalid) {
			return Promise.reject(new Error(invalid));
		}
		if (settingSource(key) !== 'user') {
			return this.engineSettings.update(key, value).then(() => this.snapshot);
		}
		const result = this.pending.then(() => this.applyAndCommit(key, value));
		this.pending = result.catch(() => { /* Each change reports its own failure. */ });
		return result;
	}

	private async applyAndCommit(key: string, value: unknown): Promise<ConfigurationSnapshot> {
		if (this.user.error) {
			throw new Error(this.user.error);
		}
		const previous = this.user.values;
		const group = engineGroup(key);
		if (group) {
			try {
				await this.apply(group, { ...previous, [key]: value });
			} catch (error) {
				await this.apply(group, previous);
				throw error;
			}
		}
		try {
			await this.user.update(key, value);
		} catch (error) {
			if (group) {
				await this.apply(group, previous);
			}
			throw error;
		}
		if (key === 'desktop.quality' && this.engine.available) {
			// The engine clamps quality limits; reapply the committed value.
			await this.apply(key, this.user.values);
		}
		return this.snapshot;
	}

	private async apply(group: string, values: Values): Promise<void> {
		if (!this.engine.available) {
			return;
		}
		for (const command of engineCommands(group, values)) {
			await this.engine.execute(command);
		}
	}

	/** Pushes every engine-backed user setting after the engine starts, then reads the engine's settings. */
	public async restoreEngineDefaults(temporaryEngine: boolean): Promise<void> {
		const values = this.user.values;
		for (const group of ['desktop.codec', 'desktop.quality', 'input']) {
			await this.apply(group, values);
		}
		if (values['media.camera']) {
			await this.apply('media.camera', values).catch((error: unknown) => {
				this.logService.error('Saved camera source could not be opened.', error);
			});
		}
		if (values['media.microphone'] || values['media.systemAudio']) {
			await this.apply('media.audio', values).catch((error: unknown) => {
				this.logService.error('Saved audio sources could not be opened.', error);
			});
		}
		try {
			const paths = fileRoots(values);
			await this.engine.execute(paths.length || temporaryEngine ? { type: 'setFileRoots', paths } : { type: 'enableFilesystemAccess' });
		} catch (error) {
			this.logService.error('Shared folders could not be opened; file access remains disabled.', error);
		}
		await this.reload();
	}
}

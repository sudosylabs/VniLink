import * as nls from '../../../nls.js';
import { Emitter, Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';
import { engineErrorCategory, isCodecPreference, isQualityLimits, type EngineCommand } from '../../engine/common/engine.js';
import { IEngineMainService } from '../../engine/electron-main/engineMainService.js';
import type { EngineMainService } from '../../engine/electron-main/engineMainService.js';
import { UserSettings } from '../node/userSettings.js';
import { settingSource, validateSetting } from '../common/configurationSchema.js';
import type { ConfigurationSnapshot } from '../common/configurationSnapshot.js';
import { EngineSettings } from './engineSettings.js';
import { SystemSettings } from './systemSettings.js';

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
			// Automatic is decided for each desktop as it opens, by the window that shows it.
			return [{ type: 'setInputDefaults', mode, swap: values['input.swapModifiers'] === 'always', pointer }];
		}
		case 'media.camera':
			return [{ type: 'setNativeCameraSources', ids: sourceIds(values['media.camera']) }];
		case 'media.audio':
			return [{ type: 'setNativeAudioSources', microphones: sourceIds(values['media.microphone']), system: sourceIds(values['media.systemAudio']) }];
		case 'diagnostics':
			return [{ type: 'setLogFilter', directives: logDirectives(values['diagnostics.engineLog'], values['diagnostics.engineLogFilter']) }];
		case 'terminal.shell': {
			// The empty id offers the automatic shell.
			const shell = values['terminal.shell'];
			return [{ type: 'setTerminalShell', shell: typeof shell === 'string' && shell ? shell : null }];
		}
	}
	return [];
}

/** The engine group a user setting belongs to; settings without one stay in this process. */
function engineGroup(key: string): string | undefined {
	if (key === 'desktop.quality' || key === 'desktop.codec' || key === 'media.camera' || key === 'terminal.shell') {
		return key;
	}
	if (key === 'media.microphone' || key === 'media.systemAudio') {
		return 'media.audio';
	}
	if (key === 'diagnostics.engineLog' || key === 'diagnostics.engineLogFilter') {
		return 'diagnostics';
	}
	return key.startsWith('input.') ? 'input' : undefined;
}

/**
 * The engine log's filter: the advanced filter when set, else the level for
 * VniLink's areas with libraries' warnings; `null` is the engine's default.
 */
function logDirectives(level: unknown, filter: unknown): string | null {
	if (typeof filter === 'string' && filter.trim()) {
		return filter.trim();
	}
	switch (level) {
		case 'off': return 'off';
		case 'error': case 'warn': case 'debug': case 'trace': return `warn,vnilink=${level}`;
		default: return null;
	}
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
	private readonly systemSettings: SystemSettings;
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
		this.systemSettings = this._register(new SystemSettings());
		this._register(Event.any(this.user.onDidChange, this.engineSettings.onDidChange, this.systemSettings.onDidChange)(() => {
			this.current = undefined;
			this.changed.fire(this.snapshot);
		}));
	}

	public get snapshot(): ConfigurationSnapshot {
		this.current ??= {
			revision: this.revision++,
			values: { ...this.user.values, ...this.engineSettings.values, ...this.systemSettings.values },
			errors: { ...this.engineSettings.sourceErrors, ...(this.user.error ? { user: this.user.error } : {}) },
			restricted: [...this.engineSettings.restricted, ...this.systemSettings.restricted],
		};
		return this.current;
	}

	/** The current value of a user setting. */
	public getValue(key: string): unknown {
		return this.user.values[key];
	}

	public async initialize(): Promise<void> {
		await Promise.all([this.user.initialize(), this.systemSettings.refresh()]);
	}

	public flush(): Promise<void> {
		return this.user.flush();
	}

	/** Reads the engine's and the operating system's settings again, for changes made outside this app. */
	public async reload(): Promise<void> {
		await Promise.all([this.engineSettings.refresh(), this.systemSettings.refresh()]);
	}

	public updateValue(key: string, value: unknown): Promise<ConfigurationSnapshot> {
		const invalid = validateSetting(key, value);
		if (invalid) {
			return Promise.reject(new Error(invalid));
		}
		if (this.systemSettings.restricted.includes(key)) {
			return Promise.reject(new Error(`${key} cannot be changed on this computer`));
		}
		if (settingSource(key) === 'system') {
			return this.systemSettings.update(key, value).then(() => this.snapshot);
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
				await this.apply(group, previous).catch(() => { /* The engine refused the change too, so it still runs the previous value. */ });
				throw group === 'terminal.shell' && engineErrorCategory(error) === 'Busy'
					? new Error(nls.localize('terminal.shellBusy', 'Close hosted terminals before changing the shell.'))
					: error;
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
		await this.apply('diagnostics', values).catch((error: unknown) => {
			this.logService.error('Saved engine log filter was refused; the engine keeps its default.', error);
		});
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
		// The engine starts with the automatic shell; a saved choice that is no
		// longer installed keeps that, and stays saved for when it returns.
		if (values['terminal.shell'] && !temporaryEngine) {
			await this.apply('terminal.shell', values).catch((error: unknown) => {
				this.logService.warn('The saved terminal shell is not available; paired computers get the automatic shell.', error);
			});
		}
		// Files shares the whole disk; whether a paired computer may browse it
		// is `sharing.files`. A temporary engine shares nothing.
		if (!temporaryEngine) {
			await this.engine.execute({ type: 'enableFilesystemAccess' }).catch((error: unknown) => {
				this.logService.error('The disk could not be opened for Files; file access remains disabled.', error);
			});
		}
		await this.reload();
	}
}

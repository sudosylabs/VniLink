import type { Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';
import { IEngineMainService } from '../../engine/electron-main/engineMainService.js';
import type { EngineMainService } from '../../engine/electron-main/engineMainService.js';
import { ConfigurationStore } from '../node/configurationStore.js';
import type { ConfigurationSnapshot, ConfigurationUpdate, ConfigurationValues } from '../common/configuration.js';

export const IConfigurationMainService = createDecorator<ConfigurationMainService>('configurationMainService');

/**
 * Owns persisted preferences and keeps the engine in step with them. A change
 * reaches the engine before it is committed, and is rolled back from the
 * engine when the commit fails, so disk and runtime never disagree.
 */
export class ConfigurationMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly store: ConfigurationStore;
	private pending: Promise<unknown> = Promise.resolve();

	constructor(
		path: string,
		@IEngineMainService private readonly engine: EngineMainService,
		@ILogService private readonly logService: ILogService,
	) {
		super();
		this.store = this._register(new ConfigurationStore(path));
	}

	public get onDidChangeConfiguration(): Event<ConfigurationSnapshot> {
		return this.store.onDidChangeConfiguration;
	}

	public get snapshot(): ConfigurationSnapshot {
		return this.store.snapshot;
	}

	public initialize(): Promise<void> {
		return this.store.initialize();
	}

	public flush(): Promise<void> {
		return this.store.flush();
	}

	public updateValue(change: ConfigurationUpdate): Promise<ConfigurationSnapshot> {
		const result = this.pending.then(() => this.applyAndCommit(change));
		this.pending = result.catch(() => { /* The committed command remains valid when readback fails. */ });
		return result;
	}

	private async applyAndCommit(change: ConfigurationUpdate): Promise<ConfigurationSnapshot> {
		const previous = this.store.snapshot;
		if (previous.error) {
			throw new Error(previous.error);
		}
		try {
			await this.applyToEngine(change.key, { ...previous.values, [change.key]: change.value });
		} catch (error) {
			await this.applyToEngine(change.key, previous.values);
			throw error;
		}
		let snapshot: ConfigurationSnapshot;
		try {
			snapshot = await this.store.updateValue(change);
		} catch (error) {
			await this.applyToEngine(change.key, previous.values);
			throw error;
		}
		if (change.key === 'desktop.quality' && this.engine.available) {
			// The engine clamps quality limits; reapply the committed value.
			await this.engine.execute({ type: 'setDesktopQualityDefaults', quality: snapshot.values['desktop.quality'] });
		}
		return snapshot;
	}

	private async applyToEngine(key: keyof ConfigurationValues, values: ConfigurationValues): Promise<void> {
		if (!this.engine.available) {
			return;
		}
		switch (key) {
			case 'desktop.quality':
				await this.engine.execute({ type: 'setDesktopQualityDefaults', quality: values['desktop.quality'] });
				break;
			case 'desktop.codec':
				await this.engine.execute({ type: 'setDesktopCodecDefault', codec: values['desktop.codec'] });
				break;
			case 'media.sources': {
				const sources = values['media.sources'];
				await this.engine.execute({ type: 'setNativeCameraSources', ids: [...sources.cameras] });
				await this.engine.execute({ type: 'setNativeAudioSources', microphones: [...sources.microphones], system: [...sources.system] });
				break;
			}
			case 'input.defaults':
				await this.engine.execute({ type: 'setInputDefaults', ...values['input.defaults'] });
				break;
			case 'files.roots': {
				const paths = values['files.roots'];
				await this.engine.execute(paths.length ? { type: 'setFileRoots', paths: [...paths] } : { type: 'enableFilesystemAccess' });
				break;
			}
			case 'recording.defaults':
			case 'appearance.theme':
			case 'appearance.nativeBackground':
				break;
		}
	}

	/** Pushes every engine-backed preference after the engine starts. */
	public async restoreEngineDefaults(temporaryEngine: boolean): Promise<void> {
		const values = this.store.snapshot.values;
		await this.engine.execute({ type: 'setDesktopCodecDefault', codec: values['desktop.codec'] });
		await this.engine.execute({ type: 'setDesktopQualityDefaults', quality: values['desktop.quality'] });
		await this.engine.execute({ type: 'setInputDefaults', ...values['input.defaults'] });
		const sources = values['media.sources'];
		if (sources.cameras.length) {
			await this.engine.execute({ type: 'setNativeCameraSources', ids: [...sources.cameras] }).catch((error: unknown) => {
				this.logService.error('Saved camera source could not be opened.', error);
			});
		}
		if (sources.microphones.length || sources.system.length) {
			await this.engine.execute({ type: 'setNativeAudioSources', microphones: [...sources.microphones], system: [...sources.system] }).catch((error: unknown) => {
				this.logService.error('Saved audio sources could not be opened.', error);
			});
		}
		try {
			const paths = values['files.roots'];
			await this.engine.execute(paths.length || temporaryEngine ? { type: 'setFileRoots', paths: [...paths] } : { type: 'enableFilesystemAccess' });
		} catch (error) {
			this.logService.error('Shared folders could not be opened; file access remains disabled.', error);
		}
	}
}

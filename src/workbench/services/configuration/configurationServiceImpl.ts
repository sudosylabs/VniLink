import { equals } from '../../../base/common/objects.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IMainProcessService } from '../../../platform/ipc/common/mainProcessService.js';
import { ConfigurationTarget, IConfigurationService, type IConfigurationChange, type IConfigurationChangeEvent, type IConfigurationData, type IConfigurationOverrides, type IConfigurationValue } from '../../../platform/configuration/common/configuration.js';
import { settingDefaults, settingKeys, settingSource } from '../../../platform/configuration/common/configurationSchema.js';
import { CONFIGURATION_CHANNEL_NAME, freezeSnapshot, isConfigurationSnapshot, type ConfigurationSnapshot } from '../../../platform/configuration/common/configurationSnapshot.js';
import type { IWorkbenchConfigurationService } from './configurationService.js';

/**
 * The window's copy of the main process's settings. There are no workspaces,
 * folders, overrides or language settings: every setting is either a user
 * setting or owned by the engine, and newer revisions replace older ones.
 */
export class ConfigurationService extends Disposable implements IWorkbenchConfigurationService {
	declare public readonly _serviceBrand: undefined;
	private readonly channel: IChannel;
	private readonly changed = this._register(new Emitter<IConfigurationChangeEvent>());
	public readonly onDidChangeConfiguration = this.changed.event;
	private snapshot: ConfigurationSnapshot = { revision: -1, values: {}, errors: {}, restricted: [] };

	constructor(@IMainProcessService mainProcessService: IMainProcessService) {
		super();
		this.channel = mainProcessService.getChannel(CONFIGURATION_CHANNEL_NAME);
		this._register(this.channel.listen<unknown>('onDidChangeConfiguration')(snapshot => {
			this.accept(snapshot);
		}));
	}

	public async initialize(): Promise<void> {
		this.accept(await this.channel.call<unknown>('getSnapshot'));
	}

	public getConfigurationData(): IConfigurationData | null {
		return null;
	}

	// The type parameter mirrors VS Code's IConfigurationService.getValue contract.
	// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters
	public getValue<T>(sectionOrOverrides?: string | IConfigurationOverrides): T {
		if (typeof sectionOrOverrides !== 'string') {
			return this.snapshot.values as T;
		}
		return this.snapshot.values[sectionOrOverrides] as T;
	}

	public async updateValue(key: string, value: unknown): Promise<void> {
		this.accept(await this.channel.call<unknown>('updateValue', { key, value }));
	}

	public inspect<T>(key: string): IConfigurationValue<Readonly<T>> {
		const value = this.snapshot.values[key] as T | undefined;
		const defaultValue = settingDefaults()[key] as T | undefined;
		const userValue = value === undefined || equals(value, defaultValue) ? undefined : value;
		return {
			value,
			defaultValue,
			userValue,
			default: defaultValue === undefined ? undefined : { value: defaultValue },
			user: userValue === undefined ? undefined : { value: userValue },
		};
	}

	/** Reads the engine's settings again, for changes made outside this window. */
	public async reloadConfiguration(): Promise<void> {
		this.accept(await this.channel.call<unknown>('reload'));
	}

	public keys(): { default: string[]; policy: string[]; user: string[]; workspace: string[]; workspaceFolder: string[] } {
		const keys = settingKeys();
		return { default: keys, policy: [], user: keys.filter(key => this.inspect(key).userValue !== undefined), workspace: [], workspaceFolder: [] };
	}

	public getUnavailableReason(key: string): string | undefined {
		return sourceError(this.snapshot, key);
	}

	public isRestricted(key: string): boolean {
		return this.snapshot.restricted.includes(key);
	}

	private accept(snapshot: unknown): void {
		if (!isConfigurationSnapshot(snapshot)) {
			throw new Error('Invalid configuration snapshot');
		}
		if (this._store.isDisposed || snapshot.revision <= this.snapshot.revision) {
			return;
		}
		const previous = this.snapshot;
		this.snapshot = freezeSnapshot(snapshot);
		// A setting changes when its value, readability or support changes.
		const keys = settingKeys().filter(key => !equals(previous.values[key], snapshot.values[key])
			|| sourceError(previous, key) !== sourceError(snapshot, key)
			|| previous.restricted.includes(key) !== snapshot.restricted.includes(key));
		if (keys.length) {
			this.changed.fire(changeEvent(keys));
		}
	}
}

function sourceError(snapshot: ConfigurationSnapshot, key: string): string | undefined {
	const source = settingSource(key);
	return source ? snapshot.errors[source] : undefined;
}

function changeEvent(keys: readonly string[]): IConfigurationChangeEvent {
	const change: IConfigurationChange = { keys: [...keys], overrides: [] };
	return {
		source: ConfigurationTarget.USER,
		affectedKeys: new Set(keys),
		change,
		affectsConfiguration: section => keys.some(key => key === section || key.startsWith(`${section}.`)),
	};
}

registerSingleton(IConfigurationService, ConfigurationService, InstantiationType.Delayed);

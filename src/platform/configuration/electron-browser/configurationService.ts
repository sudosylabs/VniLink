import { equals } from '../../../base/common/objects.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { InstantiationType, registerSingleton } from '../../instantiation/common/extensions.js';
import { ConfigurationTarget, IConfigurationService, type IConfigurationChange, type IConfigurationChangeEvent, type IConfigurationData, type IConfigurationOverrides, type IConfigurationValue } from '../common/configuration.js';
import { defaultConfiguration, IConfigurationSnapshotService, isConfigurationUpdate, type ConfigurationValues } from '../common/configurationSnapshot.js';

/**
 * VS Code's configuration contract over VniLink's preference snapshot. Every
 * preference is a user setting stored by the main process; there are no
 * workspaces, folders, overrides or policies.
 */
export class ConfigurationService extends Disposable implements IConfigurationService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<IConfigurationChangeEvent>());
	public readonly onDidChangeConfiguration = this.changed.event;
	private values: ConfigurationValues;

	constructor(@IConfigurationSnapshotService private readonly snapshots: IConfigurationSnapshotService) {
		super();
		this.values = snapshots.snapshot.values;
		this._register(snapshots.onDidChangeConfiguration(snapshot => {
			const previous = this.values;
			this.values = snapshot.values;
			const keys = Object.keys(snapshot.values).filter(key => !equals(Reflect.get(previous, key), Reflect.get(snapshot.values, key)));
			if (keys.length) {
				this.changed.fire(changeEvent(keys));
			}
		}));
	}

	public getConfigurationData(): IConfigurationData | null {
		return null;
	}

	// The type parameter mirrors VS Code's IConfigurationService.getValue contract.
	// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters
	public getValue<T>(sectionOrOverrides?: string | IConfigurationOverrides): T {
		if (typeof sectionOrOverrides !== 'string') {
			return this.values as T;
		}
		return Reflect.get(this.values, sectionOrOverrides) as T;
	}

	public async updateValue(key: string, value: unknown): Promise<void> {
		const change = { key, value };
		if (!isConfigurationUpdate(change)) {
			throw new Error(`Invalid value for setting ${key}`);
		}
		await this.snapshots.updateValue(change);
	}

	public inspect<T>(key: string): IConfigurationValue<Readonly<T>> {
		const value = Reflect.get(this.values, key) as T | undefined;
		const defaultValue = Reflect.get(defaultConfiguration, key) as T | undefined;
		return {
			value,
			defaultValue,
			userValue: equals(value, defaultValue) ? undefined : value,
			default: defaultValue === undefined ? undefined : { value: defaultValue },
			user: value === undefined || equals(value, defaultValue) ? undefined : { value },
		};
	}

	public reloadConfiguration(): Promise<void> {
		return this.snapshots.initialize();
	}

	public keys(): { default: string[]; policy: string[]; user: string[]; workspace: string[]; workspaceFolder: string[] } {
		const keys = Object.keys(defaultConfiguration);
		return { default: keys, policy: [], user: keys.filter(key => !equals(Reflect.get(this.values, key), Reflect.get(defaultConfiguration, key))), workspace: [], workspaceFolder: [] };
	}
}

function changeEvent(keys: readonly string[]): IConfigurationChangeEvent {
	const affectedKeys = new Set(keys);
	const change: IConfigurationChange = { keys: [...keys], overrides: [] };
	return {
		source: ConfigurationTarget.USER,
		affectedKeys,
		change,
		affectsConfiguration: section => keys.some(key => key === section || key.startsWith(`${section}.`)),
	};
}

registerSingleton(IConfigurationService, ConfigurationService, InstantiationType.Delayed);

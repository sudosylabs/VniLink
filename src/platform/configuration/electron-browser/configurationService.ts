import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { Emitter } from '../../../base/common/event.js';
import type { DesktopApi } from '../../ipc/common/desktopApi.js';
import { freezeConfiguration, defaultConfiguration, type IConfigurationService, type ConfigurationSnapshot, type ConfigurationUpdate } from '../common/configuration.js';

export class ConfigurationService extends Disposable implements IConfigurationService {
	declare public readonly _serviceBrand: undefined;
	private current: ConfigurationSnapshot = { revision: -1, values: defaultConfiguration, error: null };
	private readonly changed = this._register(new Emitter<ConfigurationSnapshot>());
	public readonly onDidChangeConfiguration = this.changed.event;

	constructor(private readonly api: Pick<DesktopApi, 'getConfiguration' | 'updateConfiguration' | 'onConfigurationChanged'>) {
		super();
		this._register(toDisposable(api.onConfigurationChanged(snapshot => {
			this.accept(snapshot);
		})));
	}

	public get snapshot(): ConfigurationSnapshot {
		return this.current;
	}

	public async initialize(): Promise<void> {
		this.accept(await this.api.getConfiguration());
	}

	public async updateValue(change: ConfigurationUpdate): Promise<void> {
		this.accept(await this.api.updateConfiguration(change));
	}

	private accept(snapshot: ConfigurationSnapshot): void {
		if (this._store.isDisposed || snapshot.revision <= this.current.revision) {
			return;
		}
		this.current = { ...snapshot, values: freezeConfiguration(snapshot.values) };
		this.changed.fire(this.current);
	}
}

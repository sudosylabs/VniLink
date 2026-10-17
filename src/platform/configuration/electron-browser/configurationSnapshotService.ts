import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';
import { IMainProcessService } from '../../ipc/common/mainProcessService.js';
import { CONFIGURATION_CHANNEL_NAME, defaultConfiguration, freezeConfiguration, isConfigurationSnapshot, type ConfigurationSnapshot, type ConfigurationUpdate, type IConfigurationSnapshotService } from '../common/configurationSnapshot.js';

/** Window copy of the main-process preferences; newer revisions replace older ones. */
export class ConfigurationSnapshotService extends Disposable implements IConfigurationSnapshotService {
	declare public readonly _serviceBrand: undefined;
	private readonly channel: IChannel;
	private current: ConfigurationSnapshot = { revision: -1, values: defaultConfiguration, error: null };
	private readonly changed = this._register(new Emitter<ConfigurationSnapshot>());
	public readonly onDidChangeConfiguration = this.changed.event;

	constructor(@IMainProcessService mainProcessService: IMainProcessService) {
		super();
		this.channel = mainProcessService.getChannel(CONFIGURATION_CHANNEL_NAME);
		this._register(this.channel.listen<unknown>('onDidChangeConfiguration')(snapshot => {
			this.accept(snapshot);
		}));
	}

	public get snapshot(): ConfigurationSnapshot {
		return this.current;
	}

	public async initialize(): Promise<void> {
		this.accept(await this.channel.call<unknown>('getSnapshot'));
	}

	public async updateValue(change: ConfigurationUpdate): Promise<void> {
		this.accept(await this.channel.call<unknown>('updateValue', change));
	}

	private accept(snapshot: unknown): void {
		if (!isConfigurationSnapshot(snapshot)) {
			throw new Error('Invalid configuration snapshot');
		}
		if (this._store.isDisposed || snapshot.revision <= this.current.revision) {
			return;
		}
		this.current = { ...snapshot, values: freezeConfiguration(snapshot.values) };
		this.changed.fire(this.current);
	}
}

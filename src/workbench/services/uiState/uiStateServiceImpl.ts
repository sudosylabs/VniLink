import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IMainProcessService } from '../../../platform/ipc/common/mainProcessService.js';
import { isUIStateValue, UI_STATE_CHANNEL_NAME, type UIStateKey, type UIStateValues } from '../../../platform/state/common/uiState.js';
import { IUIStateService } from './uiStateService.js';

export class UIStateService implements IUIStateService {
	declare public readonly _serviceBrand: undefined;
	private readonly channel: IChannel;

	constructor(@IMainProcessService mainProcessService: IMainProcessService) {
		this.channel = mainProcessService.getChannel(UI_STATE_CHANNEL_NAME);
	}

	public async get<K extends UIStateKey>(key: K): Promise<UIStateValues[K] | undefined> {
		const value = await this.channel.call<unknown>('get', { key });
		// The state file outlives this version, so what it holds is checked like any input.
		return isUIStateValue(key, value) ? value : undefined;
	}

	public async set<K extends UIStateKey>(key: K, value: UIStateValues[K] | undefined): Promise<void> {
		await (value === undefined ? this.channel.call('remove', { key }) : this.channel.call('set', { key, value }));
	}
}

registerSingleton(IUIStateService, UIStateService, InstantiationType.Delayed);

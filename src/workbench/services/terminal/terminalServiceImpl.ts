import { DataChannelClient } from '../../../platform/engine/electron-browser/dataChannelClient.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import type { ITerminalChildProcess } from '../../../platform/terminal/common/terminal.js';
import { EnginePty, type TerminalChannelOpener } from './enginePty.js';
import { ITerminalService } from './terminalService.js';

export class TerminalService implements ITerminalService {
	declare public readonly _serviceBrand: undefined;
	private readonly openChannel: TerminalChannelOpener;

	constructor(
		@IEngineService engine: IEngineService,
		@IInstantiationService private readonly instantiationService: IInstantiationService,
	) {
		this.openChannel = (request, consume, onError) => DataChannelClient.open(engine, request, consume, onError);
	}

	public createProcess(deviceId: string, cols: number, rows: number): ITerminalChildProcess {
		return this.instantiationService.createInstance(EnginePty, deviceId, cols, rows, this.openChannel);
	}
}

registerSingleton(ITerminalService, TerminalService, InstantiationType.Delayed);

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

/** The relays the running engine uses; saved relays apply only after a restart. */
export interface RunningRelays {
	readonly publicRelay: boolean;
	readonly urls: readonly string[];
}

export const IConnectionService = createDecorator<IConnectionService>('connectionService');

export interface IConnectionService {
	readonly _serviceBrand: undefined;
	getRunningRelays(): Promise<RunningRelays>;
}

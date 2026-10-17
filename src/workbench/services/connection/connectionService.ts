import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { RelayInfo, RelayInput } from '../../../platform/engine/common/engine.js';

export interface ConnectionSettings {
	readonly restartRequired: boolean;
	readonly saved: readonly Readonly<RelayInfo>[];
	readonly active: { readonly publicRelay: boolean; readonly urls: readonly string[] };
}

export type ConnectionUpdate = { readonly mode: 'public' } | { readonly mode: 'custom'; readonly relays: readonly RelayInput[] };

export const IConnectionService = createDecorator<IConnectionService>('connectionService');

export interface IConnectionService {
	readonly _serviceBrand: undefined;
	readonly available: boolean;
	load(): Promise<ConnectionSettings>;
	save(update: ConnectionUpdate): Promise<ConnectionSettings>;
}
/** A committed save must not be reported as a failed write if the read-back fails. */
export class ConnectionReadbackError extends Error {
	constructor() {
		super('Relay configuration was saved, but could not be reloaded');
	}
}

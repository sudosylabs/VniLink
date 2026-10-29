import type { Event } from '../../../base/common/event.js';
import type { IServerChannel } from '../../../base/parts/ipc/common/ipc.js';
import { windowIdFromContext } from '../../ipc/electron-main/windowContext.js';
import { isConfigurationUpdate } from '../common/configurationSnapshot.js';
import type { ConfigurationMainService } from './configurationMainService.js';

/** Serves setting snapshots and validated setting changes to windows. */
export class ConfigurationChannel implements IServerChannel {

	constructor(private readonly configuration: ConfigurationMainService) { }

	public listen<T>(_context: string, event: string): Event<T> {
		if (event === 'onDidChangeConfiguration') {
			return this.configuration.onDidChangeConfiguration as Event<T>;
		}
		throw new Error(`Unknown configuration event: ${event}`);
	}

	public async call<T>(context: string, command: string, arg?: unknown): Promise<T> {
		windowIdFromContext(context);
		switch (command) {
			case 'getSnapshot':
				return this.configuration.snapshot as T;
			case 'reload':
				await this.configuration.reload();
				return this.configuration.snapshot as T;
			case 'updateValue':
				if (!isConfigurationUpdate(arg)) {
					throw new Error('Invalid configuration update');
				}
				return await this.configuration.updateValue(arg.key, arg.value) as T;
		}
		throw new Error(`Unknown configuration command: ${command}`);
	}
}

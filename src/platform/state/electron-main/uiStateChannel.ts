import type { Event } from '../../../base/common/event.js';
import type { IServerChannel } from '../../../base/parts/ipc/common/ipc.js';
import { windowIdFromContext } from '../../ipc/electron-main/windowContext.js';
import { isUIStateRead, isUIStateWrite } from '../common/uiState.js';
import type { IStateService } from '../node/state.js';

/**
 * Serves windows the UI state they remember between runs, such as the Files
 * tab's last folders. Only the key families of `UIStateValues` are reachable,
 * each with its own value shape; the rest of the state file stays the main
 * process's.
 */
export class UIStateChannel implements IServerChannel {

	constructor(private readonly state: IStateService) { }

	public listen<T>(_context: string, event: string): Event<T> {
		throw new Error(`Unknown UI state event: ${event}`);
	}

	public call<T>(context: string, command: string, arg?: unknown): Promise<T> {
		// The executor turns a refusal into a rejected call.
		return new Promise(resolve => {
			resolve(this.execute(context, command, arg) as T);
		});
	}

	private execute(context: string, command: string, arg: unknown): unknown {
		windowIdFromContext(context);
		switch (command) {
			case 'get':
				if (!isUIStateRead(arg)) {
					throw new Error('Invalid UI state key');
				}
				return this.state.getItem(arg.key);
			case 'set':
				if (!isUIStateWrite(arg)) {
					throw new Error('Invalid UI state value');
				}
				this.state.setItem(arg.key, arg.value);
				return undefined;
			case 'remove':
				if (!isUIStateRead(arg)) {
					throw new Error('Invalid UI state key');
				}
				this.state.removeItem(arg.key);
				return undefined;
		}
		throw new Error(`Unknown UI state command: ${command}`);
	}
}

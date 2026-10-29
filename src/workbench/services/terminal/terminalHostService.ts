import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { TerminalShells } from '../../../platform/engine/common/engine.js';

export const ITerminalHostService = createDecorator<ITerminalHostService>('terminalHostService');

/** What this computer offers paired computers' terminals. */
export interface ITerminalHostService {
	readonly _serviceBrand: undefined;
	/** The shells installed here, what automatic resolves to, and the one offered now. */
	shells(): Promise<TerminalShells>;
}

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { ITerminalChildProcess } from '../../../platform/terminal/common/terminal.js';

export const ITerminalService = createDecorator<ITerminalService>('terminalService');

/** Shells on paired computers. Each process belongs to the view that created it. */
export interface ITerminalService {
	readonly _serviceBrand: undefined;
	/** A shell on `deviceId`, not yet started, sized to the view's grid. */
	createProcess(deviceId: string, cols: number, rows: number): ITerminalChildProcess;
}

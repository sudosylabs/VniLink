import type { Event } from '../../../base/common/event.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import type { DataChannelRequest } from './dataChannel.js';
import type { EngineCommand, EngineReply } from './engine.js';

/** Main-process IPC channel that carries engine commands for a window. */
export const ENGINE_CHANNEL_NAME = 'engine';

/** Response channel on which main transfers a data-channel `MessagePort` to the requesting window. */
export const DATA_CHANNEL_PORT_RESPONSE = 'vscode:dataChannelPort';

export const IEngineService = createDecorator<IEngineService>('engineService');

/**
 * Window access to the Rust engine. Commands are validated again in main and
 * in Rust; this contract only names what the workbench may ask for.
 */
export interface IEngineService {
	readonly _serviceBrand: undefined;

	/** Whether the engine accepted commands when this window last heard from main. */
	readonly available: boolean;
	readonly onDidChangeAvailability: Event<boolean>;

	/** Whether relay changes need an application restart to take effect. */
	readonly restartRequired: boolean;
	readonly onDidChangeRestartRequired: Event<boolean>;

	execute(command: EngineCommand): Promise<EngineReply>;

	/** Asks main to attach a native data channel; the port arrives on {@link DATA_CHANNEL_PORT_RESPONSE}. */
	openDataChannel(request: DataChannelRequest): Promise<void>;
}

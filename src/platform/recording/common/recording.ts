import { createDecorator } from '../../instantiation/common/instantiation.js';

/** Main-process IPC channel serving {@link IRecordingService}. */
export const RECORDING_CHANNEL_NAME = 'recording';

export const IRecordingService = createDecorator<IRecordingService>('recordingService');

/** Where session recordings are written on this computer. */
export interface IRecordingService {
	readonly _serviceBrand: undefined;

	/** The folder used when the user has not chosen one. */
	getDefaultDirectory(): Promise<string>;

	/** Creates the recordings folder if needed and returns a new, unused file path in it. */
	createDestination(): Promise<string>;
}

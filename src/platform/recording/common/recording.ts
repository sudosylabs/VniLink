import type { VSBuffer } from '../../../base/common/buffer.js';
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

	/**
	 * Saves a PNG picture as `name` in the recordings folder, next to
	 * recordings, as a camera's shutter does; a taken name gets a number.
	 * The path stays in the main process.
	 */
	savePicture(name: string, png: VSBuffer): Promise<void>;
}

/** A file name, not a path: the window names a picture and the main process places it. */
export function isPictureName(value: unknown): value is string {
	return typeof value === 'string' && value.length > 4 && value.length <= 200 && value.endsWith('.png')
		&& !/[\\/:\0<>"|?*]/.test(value) && !value.startsWith('.');
}

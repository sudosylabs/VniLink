import type { Event } from '../../../base/common/event.js';
import type { IDisposable } from '../../../base/common/lifecycle.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { RecordingStartFailure } from '../recording/sessionRecording.js';

export const ICameraService = createDecorator<ICameraService>('cameraService');

/** Why a paired computer's camera can't be opened. */
export type CameraFailure =
	/** This computer isn't connected to it. */
	| 'disconnected'
	/** Its user doesn't share the camera, with this computer or at all. */
	| 'turnedOff'
	/** Its system hasn't allowed VniLink to use the camera. */
	| 'needsPermission'
	/** It has no camera support. */
	| 'unsupported'
	/** It can't open another service right now. */
	| 'unavailable'
	/** It shares its camera but has none connected. */
	| 'noCamera'
	/** Another computer is watching this camera. */
	| 'inUse'
	/** The camera didn't start. */
	| 'failed';

/** Why a camera picture stopped. */
export type CameraEnd =
	/** The paired computer's camera stopped delivering pictures. */
	| 'stopped'
	/** Its user stopped sharing the camera. */
	| 'withdrawn'
	/** The connection ended. */
	| 'lost'
	/** This computer can't decode the picture. */
	| 'cantShow';

export interface CameraChoice {
	readonly id: string;
	readonly name: string;
}

/** The cameras a paired computer offers, and whether it lets this computer record them. */
export interface CameraList {
	readonly cameras: readonly CameraChoice[];
	readonly recording: boolean;
}


/**
 * A running camera picture. Each frame belongs to the listener, which closes
 * it once drawn; nothing else holds frames.
 */
export interface ICameraStream extends IDisposable {
	readonly onFrame: Event<VideoFrame>;
	/** Fires once; disposing the stream first fires nothing. */
	readonly onEnd: Event<CameraEnd>;
}

/** The cameras of paired computers. A camera has one viewer at a time. */
export interface ICameraService {
	readonly _serviceBrand: undefined;
	/** The cameras `deviceId` offers this computer. */
	cameras(deviceId: string): Promise<{ readonly failure: CameraFailure } | CameraList>;
	/** Starts the picture of one of them. */
	open(deviceId: string, cameraId: string): Promise<{ readonly failure: CameraFailure } | { readonly stream: ICameraStream }>;
	/**
	 * Records the open camera of `deviceId` into the recordings folder, with
	 * the recording settings. The recording is the engine's: follow it with
	 * `INativeRuntimeService.recordingStatus` and `stopRecording`.
	 */
	startRecording(deviceId: string): Promise<{ readonly failure: RecordingStartFailure } | { readonly recording: string }>;
}

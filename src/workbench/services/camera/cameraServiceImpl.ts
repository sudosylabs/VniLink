// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { DataChannelClient } from '../../../platform/engine/electron-browser/dataChannelClient.js';
import type { DataChannelRequest, DataPacket } from '../../../platform/engine/common/dataChannel.js';
import { engineErrorCategory, type RemoteStatus } from '../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { activeControllerSession } from '../devices/controllerSession.js';
import { IDeviceService } from '../devices/deviceService.js';
import { INativeRuntimeService } from '../nativeRuntime/nativeRuntimeService.js';
import { ICameraService, type CameraFailure, type CameraList, type ICameraStream } from './cameraService.js';
import { recordingStartFailure, type RecordingStartFailure } from '../recording/sessionRecording.js';
import { EngineCameraStream } from './engineCameraStream.js';
import { createWebCodecsDecoder } from './webCodecsDecoder.js';

/**
 * The picture asked of the host, which picks its camera's nearest format.
 * Fixed until there is a quality setting.
 */
const requestedPicture = { width: 1280, height: 720, fps: 30 } as const;

export type CameraChannelOpener = (
	request: Omit<DataChannelRequest, 'requestId'>,
	consume: (packet: DataPacket) => Promise<void>,
	onError: (error: Error) => void,
) => Promise<{ close(): void }>;

/** What the host reports about its camera, as a reason it can't be opened. */
export function cameraRefusal(status: RemoteStatus | undefined): CameraFailure | undefined {
	switch (status) {
		case 'available': return undefined;
		case 'turnedOff': return 'turnedOff';
		case 'needsPermission': return 'needsPermission';
		case 'unavailable': return 'unavailable';
		case 'unsupported':
		case undefined: return 'unsupported';
	}
}

/** Why the host refused to open the camera, from the engine's error category. */
export function cameraOpenFailure(error: unknown): CameraFailure {
	switch (engineErrorCategory(error) ?? '') {
		case 'Busy': return 'inUse';
		case 'Unauthorized': return 'turnedOff';
		case 'NoSession':
		case 'StaleSession': return 'disconnected';
		default: return 'failed';
	}
}

export class CameraService implements ICameraService {
	declare public readonly _serviceBrand: undefined;
	private readonly openChannel: CameraChannelOpener;

	constructor(
		@IEngineService private readonly engine: IEngineService,
		@IDeviceService private readonly devices: IDeviceService,
		@INativeRuntimeService private readonly nativeRuntime: INativeRuntimeService,
	) {
		this.openChannel = (request, consume, onError) => DataChannelClient.open(engine, request, consume, onError);
	}

	public async cameras(deviceId: string): Promise<{ readonly failure: CameraFailure } | CameraList> {
		let report;
		try {
			report = await this.devices.availability(deviceId);
		} catch {
			return { failure: 'failed' };
		}
		if (!report) {
			return { failure: 'disconnected' };
		}
		const refused = cameraRefusal(report.find(entry => entry.capability === 'camera')?.status);
		if (refused) {
			return { failure: refused };
		}
		const recording = report.find(entry => entry.capability === 'recording')?.status === 'available';
		const session = await activeControllerSession(this.engine, deviceId);
		if (!session) {
			return { failure: 'disconnected' };
		}
		try {
			const cameras = await this.nativeRuntime.remoteDevices(session, 'camera');
			return cameras.length ? { cameras: cameras.map(camera => ({ id: camera.id, name: camera.name })), recording } : { failure: 'noCamera' };
		} catch (error) {
			return { failure: cameraOpenFailure(error) };
		}
	}

	public async open(deviceId: string, cameraId: string): Promise<{ readonly failure: CameraFailure } | { readonly stream: ICameraStream }> {
		const session = await activeControllerSession(this.engine, deviceId);
		if (!session) {
			return { failure: 'disconnected' };
		}
		const stream = new EngineCameraStream(createWebCodecsDecoder);
		try {
			const channel = await this.openChannel(
				{ kind: 'camera', sessionId: session.id, generation: session.generation, media: { type: 'camera', deviceId: cameraId, ...requestedPicture } },
				packet => stream.consume(packet),
				() => {
					stream.lost();
				},
			);
			stream.attach(channel);
		} catch (error) {
			stream.dispose();
			return { failure: cameraOpenFailure(error) };
		}
		return { stream };
	}

	public async startRecording(deviceId: string): Promise<{ readonly failure: RecordingStartFailure } | { readonly recording: string }> {
		const session = await activeControllerSession(this.engine, deviceId);
		if (!session) {
			return { failure: 'disconnected' };
		}
		try {
			const status = await this.nativeRuntime.startRecording({ ...session, source: 'camera' }, {});
			return { recording: status.id };
		} catch (error) {
			return { failure: recordingStartFailure(error) };
		}
	}
}

registerSingleton(ICameraService, CameraService, InstantiationType.Delayed);

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { NativeBackend, NativeCapability, NativeDevice, PowerStatus, NativeScreenSource, NativeSharingStatus, RecordingStatus, RecordingRetention } from '../../../platform/engine/common/engine.js';

export interface NativeRuntime { readonly backends: readonly NativeBackend[]; readonly capabilities: readonly NativeCapability[]; }

export const INativeRuntimeService = createDecorator<INativeRuntimeService>('nativeRuntimeService');

export interface INativeRuntimeService {
	readonly _serviceBrand: undefined;
	readonly available: boolean;
	recordingDirectory(action: 'read' | 'choose'): Promise<string | null>;
	load(): Promise<NativeRuntime>;
	captureDisplays(): Promise<readonly NativeScreenSource[]>;
	startSharing(source: NativeScreenSource): Promise<readonly NativeSharingStatus[]>;
	stopSharing(): Promise<readonly NativeSharingStatus[]>;
	sharingStatus(): Promise<readonly NativeSharingStatus[]>;
	startRecording(target: { readonly id: string; readonly generation: string }, options: { readonly destination?: string; readonly maxBytes?: string; readonly maxDurationMs?: string; readonly retention?: RecordingRetention }): Promise<Readonly<RecordingStatus>>;
	recordingStatus(id: string): Promise<Readonly<RecordingStatus>>;
	stopRecording(id: string): Promise<Readonly<RecordingStatus>>;
	releaseRecording(id: string): Promise<void>;
	remoteDevices(target: { readonly id: string; readonly generation: string }, kind: 'camera' | 'microphone' | 'system'): Promise<readonly NativeDevice[]>;
	devices(kind: 'camera' | 'audio' | 'output'): Promise<readonly NativeDevice[]>;
	powerStatus(): Promise<readonly PowerStatus[]>;
}

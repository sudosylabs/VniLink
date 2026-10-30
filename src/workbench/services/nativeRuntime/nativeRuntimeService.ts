// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { NativeBackend, NativeCapability, NativeDevice, PowerStatus, NativeScreenSource, NativeSharingStatus, RecordingStatus, RecordingRetention } from '../../../platform/engine/common/engine.js';

/** A connection, and which of its streams to record: its desktop unless said otherwise. */
export interface RecordingTarget { readonly id: string; readonly generation: string; readonly source?: 'desktop' | 'camera'; }

/** Per-recording overrides of the recording settings. */
export interface RecordingOptions {
	readonly destination?: string;
	readonly maxBytes?: string;
	readonly maxDurationMs?: string;
	readonly retention?: RecordingRetention;
}

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
	startRecording(target: RecordingTarget, options: RecordingOptions): Promise<Readonly<RecordingStatus>>;
	recordingStatus(id: string): Promise<Readonly<RecordingStatus>>;
	stopRecording(id: string): Promise<Readonly<RecordingStatus>>;
	releaseRecording(id: string): Promise<void>;
	remoteDevices(target: { readonly id: string; readonly generation: string }, kind: 'camera' | 'microphone' | 'system'): Promise<readonly NativeDevice[]>;
	devices(kind: 'camera' | 'audio' | 'output'): Promise<readonly NativeDevice[]>;
	powerStatus(): Promise<readonly PowerStatus[]>;
}

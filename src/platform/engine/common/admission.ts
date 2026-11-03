// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { DataChannelRequest } from './dataChannel.js';

/** A service the engine asks another computer to be admitted to, by the engine's name for it. */
export type AdmissionService = 'Desktop' | 'Files' | 'Terminal' | 'Camera' | 'SystemAudio' | 'Microphone' | 'System';

/** What to be admitted to before a use of another computer begins, on which connection. */
export interface Admission {
	readonly service: AdmissionService;
	readonly sessionId: string;
	readonly generation: string;
}

/**
 * What a data channel asks its host to admit first. A screenshot asks
 * nothing: a host that asks each time refuses it until its person accepted
 * this computer for something else.
 */
export function channelAdmission(request: DataChannelRequest): Admission | undefined {
	const service = channelServices[request.kind] ?? (request.media?.type === 'audio' ? request.media.sourceKind === 'System' ? 'SystemAudio' : 'Microphone' : undefined);
	return service && { service, sessionId: request.sessionId, generation: request.generation };
}

const channelServices: Partial<Record<DataChannelRequest['kind'], AdmissionService>> = {
	video: 'Desktop',
	input: 'Desktop',
	files: 'Files',
	terminal: 'Terminal',
	camera: 'Camera',
};

/**
 * What a command asks its host to admit first, for the commands that begin
 * a use of another computer: its devices' lists, its terminals, an action
 * or privacy mode. Following up on, cancelling or closing what was already
 * admitted asks nothing.
 */
export function commandAdmission(command: Record<string, unknown>): Admission | undefined {
	const on = (service: AdmissionService, sessionId: unknown): Admission | undefined =>
		typeof sessionId === 'string' && typeof command.generation === 'string' ? { service, sessionId, generation: command.generation } : undefined;
	switch (command.type) {
		case 'remoteCameraDevices': return on('Camera', command.sessionId);
		case 'remoteAudioDevices': return on(command.kind === 'System' ? 'SystemAudio' : 'Microphone', command.sessionId);
		case 'remoteTerminalSessions': return on('Terminal', command.connectionId);
		case 'systemAction': case 'systemPrivacy': case 'setSystemPrivacy': return on('System', command.sessionId);
		default: return undefined;
	}
}

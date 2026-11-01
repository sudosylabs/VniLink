// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import type { DesktopAudioStatus } from '../../../platform/engine/common/dataChannel.js';
import type { DisplayInfo, RecordingStatus } from '../../../platform/engine/common/engine.js';
import type { RecordingStartFailure } from '../../services/recording/sessionRecording.js';
import type { DesktopEnd, DesktopFailure } from '../../services/remoteDesktop/remoteDesktopService.js';

export type DesktopProblem = DesktopFailure | DesktopEnd;

/** Problems another attempt may fix, tried again by themselves; the rest need a change on either computer. */
export const retryable: ReadonlySet<DesktopProblem> = new Set(['disconnected', 'unavailable', 'failed', 'linkLost', 'relayUnreachable', 'peerClosed', 'hostEncoderFailed', 'hostCaptureFailed']);

/** Ends of a running picture that are tried again at once, before any failed attempt. */
export const reconnectAtOnce: ReadonlySet<DesktopProblem> = new Set(['linkLost', 'relayUnreachable', 'peerClosed', 'hostEncoderFailed', 'hostCaptureFailed']);

export function problemMessage(name: string, problem: DesktopProblem): string {
	switch (problem) {
		case 'disconnected': return nls.localize('desktop.disconnected', 'Not connected to {0}.', name);
		case 'turnedOff': return nls.localize('desktop.turnedOff', '{0} doesn’t share its screen with this computer.', name);
		case 'needsPermission': return nls.localize('desktop.needsPermission', '{0} hasn’t allowed VniLink to share its screen.', name);
		case 'unsupported': return nls.localize('desktop.unsupported', '{0} can’t share its screen.', name);
		case 'unavailable': return nls.localize('desktop.unavailable', '{0} can’t open its screen right now.', name);
		case 'failed': return nls.localize('desktop.failed', 'The screen of {0} didn’t start.', name);
		case 'linkLost': return nls.localize('desktop.lost', 'The connection to {0} dropped.', name);
		case 'relayUnreachable': return nls.localize('desktop.relayUnreachable', 'The relayed connection to {0} dropped.', name);
		case 'peerClosed': return nls.localize('desktop.peerClosed', '{0} stopped sending its screen.', name);
		case 'hostEncoderFailed': return nls.localize('desktop.hostEncoderFailed', '{0} couldn’t keep encoding its screen.', name);
		case 'hostCaptureFailed': return nls.localize('desktop.hostCaptureFailed', '{0} stopped capturing its screen.', name);
		case 'hostPermission': return nls.localize('desktop.hostPermission', '{0} no longer allows VniLink to record its screen.', name);
		case 'displayGone': return nls.localize('desktop.displayGone', 'The display shown from {0} went away.', name);
		case 'hostStopped': return nls.localize('desktop.hostStopped', '{0} stopped sharing its screen.', name);
		case 'hostDisconnected': return nls.localize('desktop.hostDisconnected', '{0} ended the connection.', name);
		case 'hostShutdown': return nls.localize('desktop.hostShutdown', 'VniLink quit on {0}.', name);
		case 'revoked': return nls.localize('desktop.revoked', '{0} no longer shares its screen with this computer.', name);
		case 'cantShow': return nls.localize('desktop.cantShow', 'This computer can’t show the screen of {0}.', name);
	}
}

/** What the user can do about a problem, when the title alone doesn't say. */
export function problemAdvice(name: string, problem: DesktopProblem): string {
	switch (problem) {
		case 'hostPermission': return nls.localize('desktop.hostPermissionAdvice', 'Someone at {0} needs to allow VniLink to record the screen in its system settings. Then try again.', name);
		case 'displayGone': return nls.localize('desktop.displayGoneAdvice', 'Try again to show another display.');
		case 'hostShutdown': return nls.localize('desktop.hostShutdownAdvice', 'Try again once VniLink is open on {0}.', name);
		case 'disconnected': case 'turnedOff': case 'needsPermission': case 'unsupported': case 'unavailable': case 'failed':
		case 'linkLost': case 'relayUnreachable': case 'peerClosed': case 'hostEncoderFailed': case 'hostCaptureFailed':
		case 'hostStopped': case 'hostDisconnected': case 'revoked': case 'cantShow':
			return '';
	}
}

/**
 * Why the other computer's sound isn't heard, from the engine's error
 * category and whether this computer's own output failed.
 */
export function soundUnavailableReason(name: string, sound: DesktopAudioStatus): string {
	if (sound.playbackFailed) {
		return nls.localize('desktop.soundPlaybackFailed', 'This computer’s speakers couldn’t play the sound of {0}.', name);
	}
	switch (sound.error?.code) {
		case 'Unauthorized': return nls.localize('desktop.soundNotShared', '{0} doesn’t share its sound.', name);
		case 'Busy': return nls.localize('desktop.soundBusy', '{0} is playing sound from another computer.', name);
		case 'Unsupported': return nls.localize('desktop.soundUnsupported', '{0} has no sound to share. On a Mac, VniLink needs Screen & System Audio Recording.', name);
		case undefined: default: return nls.localize('desktop.soundStopped', 'The sound of {0} stopped.', name);
	}
}

/** Why a reconnect is under way, as the overlay's body says it. */
export function reconnectReason(name: string, problem: DesktopProblem | undefined): string {
	switch (problem) {
		case 'hostEncoderFailed': case 'hostCaptureFailed': case 'peerClosed': return `${problemMessage(name, problem)} ${nls.localize('desktop.reconnectingHost', 'Keys and the pointer stay here until its screen is back.')}`;
		case 'relayUnreachable': return nls.localize('desktop.reconnectingRelay', 'The relayed connection dropped. Keys and the pointer stay here until it’s back.');
		case 'disconnected': case 'turnedOff': case 'needsPermission': case 'unsupported': case 'unavailable': case 'failed':
		case 'linkLost': case 'hostPermission': case 'displayGone': case 'hostStopped': case 'hostDisconnected': case 'hostShutdown': case 'revoked': case 'cantShow':
		case undefined:
			return nls.localize('desktop.reconnectingBody', 'The connection dropped. Keys and the pointer stay here until it’s back.');
	}
}

/** A display as its menu names it: its place among the other computer's displays, and its size. */
export function displayLabel(display: DisplayInfo, index: number, withSize = true): string {
	return withSize
		? nls.localize('desktop.displayWithSize', 'Display {0} · {1} × {2}', index + 1, display.width, display.height)
		: nls.localize('desktop.displayNumber', 'Display {0}', index + 1);
}

export function recordingStartMessage(name: string, failure: RecordingStartFailure): string {
	switch (failure) {
		case 'notAllowed': return nls.localize('desktop.recordingNotAllowed', '{0} doesn’t allow recording its screen.', name);
		case 'disconnected': return nls.localize('desktop.recordingDisconnected', '{0} isn’t connected.', name);
		case 'failed': return nls.localize('desktop.recordingFailed', 'The recording didn’t start.');
	}
}

export function recordingEndMessage(status: RecordingStatus | undefined): string {
	switch (status?.failure) {
		case undefined: return nls.localize('desktop.recordingLost', 'The recording stopped.');
		case null: return nls.localize('desktop.recordingSaved', 'Recording saved to the recordings folder.');
		case 'Startup': return nls.localize('desktop.recordingStartup', 'The recording didn’t start.');
		case 'Storage': return nls.localize('desktop.recordingStorage', 'The recordings folder can’t be written to.');
		case 'SlowStorage': return nls.localize('desktop.recordingSlow', 'The recordings folder couldn’t keep up, so the recording stopped.');
		case 'Limit': return nls.localize('desktop.recordingLimit', 'The recording reached its size limit.');
		case 'Media':
		case 'FormatChanged': return nls.localize('desktop.recordingMedia', 'The screen changed size, so the recording stopped.');
	}
}

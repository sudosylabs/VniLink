// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IContextMenuDelegate } from '../../../../../base/browser/contextmenu.js';
import type { IAction } from '../../../../../base/common/actions.js';
import { Emitter } from '../../../../../base/common/event.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import type { IContextMenuMenuDelegate, IContextMenuService } from '../../../../../platform/contextview/browser/contextView.js';
import type { DesktopAudioOperation, DesktopAudioStatus, DesktopCondition, DesktopMediaStatus, DesktopRateLimit } from '../../../../../platform/engine/common/dataChannel.js';
import type { DisplayInfo, RecordingStatus } from '../../../../../platform/engine/common/engine.js';
import type { INativeHostService, IWindowState } from '../../../../../platform/native/common/native.js';
import type { UIStateKey, UIStateValues } from '../../../../../platform/state/common/uiState.js';
import type { INativeRuntimeService } from '../../../../services/nativeRuntime/nativeRuntimeService.js';
import type { DesktopEnd, DesktopFailure, DesktopInput, DesktopPicture, DesktopPictureRole, DesktopPointer, DesktopRegion, DesktopStats, IDesktopSession, IRemoteDesktopService } from '../../../../services/remoteDesktop/remoteDesktopService.js';
import type { SystemPrivacyResult } from '../../../../services/systemActions/systemActionService.js';
import { TestSystemActionService } from '../../../../services/systemActions/test/common/testSystemActionService.js';
import type { IUIStateService } from '../../../../services/uiState/uiStateService.js';

/** A display as the other computer arranges it: the second one, right of the first. */
export const display: DisplayInfo = { id: '7', origin_x: 1920, origin_y: 0, width: 1920, height: 1080, rotation_deg: 0, scale_percent: 100, revision: '3' };

/** Another display of the same computer, to switch to. */
export const otherDisplay: DisplayInfo = { id: '8', origin_x: 3840, origin_y: 0, width: 2560, height: 1440, rotation_deg: 0, scale_percent: 100, revision: '3' };

/** A session whose pictures, pointer and end the test plays, and whose input it reads. */
export class TestDesktopSession implements IDesktopSession {
	private readonly pictures = new Emitter<DesktopPicture>();
	public readonly onPicture = this.pictures.event;
	private readonly ends = new Emitter<DesktopEnd>();
	public readonly onEnd = this.ends.event;
	private readonly pointers = new Emitter<DesktopPointer>();
	public readonly onPointer = this.pointers.event;
	public readonly sent: DesktopInput[] = [];
	public readonly presentedPictures: DesktopPicture[] = [];
	public readonly skippedPictures: DesktopPicture[] = [];
	public disposed = false;
	/** What the engine says is wrong with the picture, most severe first. */
	public conditions: DesktopCondition[] = ['link.congestion', 'relay.path'];
	/** Why the quality is where it is, and what holds the bitrate. */
	public reason: DesktopMediaStatus['reason'] = 'networkPressure';
	public limit: DesktopRateLimit = 'none';
	/** The other computer's sound, as its channel answers; each operation asked. */
	public sound: DesktopAudioStatus = { phase: 'playing', enabled: true, muted: false, playbackFailed: false, error: null };
	public readonly soundOperations: DesktopAudioOperation[] = [];

	constructor(public readonly control: boolean) { }

	public displays(): Promise<readonly DisplayInfo[]> {
		return Promise.resolve([display, otherDisplay]);
	}

	public audio(operation: DesktopAudioOperation): Promise<DesktopAudioStatus | undefined> {
		this.soundOperations.push(operation);
		if (operation.type === 'muted') {
			this.sound = { ...this.sound, muted: operation.muted, phase: operation.muted ? 'muted' : 'playing' };
		} else if (operation.type === 'enabled' && operation.enabled) {
			this.sound = { ...this.sound, phase: 'playing', muted: false, playbackFailed: false, error: null };
		}
		return Promise.resolve(this.sound);
	}

	public stats(): Promise<DesktopStats | undefined> {
		return Promise.resolve({ codec: 'h264', softwareDecoder: false, status: { reason: this.reason, receivedKbps: 9400, receivedFps: 60, receiverQueueMs: 4, hostQueueMs: 2, lossPerMille: 3, rttMs: 18, encodeUs: 4000, captureUs: 1000, limit: this.limit, conditions: this.conditions }, latency: { host: { p50: 9, p95: 14, count: 60 }, decode: { p50: 3, p95: 5, count: 60 } }, skipped: 0 });
	}

	public send(input: DesktopInput): void {
		if (this.control) {
			this.sent.push(input);
		}
	}

	public presented(picture: DesktopPicture): void {
		this.presentedPictures.push(picture);
	}

	public skipped(picture: DesktopPicture): void {
		this.skippedPictures.push(picture);
	}

	/** A 1920 × 1080 picture of display `shows` at revision 3, in one colour, with a 40 × 40 patch of another at 100, 100. */
	public picture(role: DesktopPictureRole = 'changed', colour = '#1d4b6c', patch?: string, regions?: readonly DesktopRegion[], shows = '7'): void {
		const canvas = new OffscreenCanvas(1920, 1080);
		const context = canvas.getContext('2d');
		if (context) {
			context.fillStyle = colour;
			context.fillRect(0, 0, 1920, 1080);
			if (patch) {
				context.fillStyle = patch;
				context.fillRect(100, 100, 40, 40);
			}
		}
		this.pictures.fire({ frame: new VideoFrame(canvas, { timestamp: 0 }), role, regions, display: shows, revision: '3', decodedAt: performance.now() });
	}

	/** A picture of display 7 at revision 3 as given, such as a painted scene. */
	public firePicture(frame: VideoFrame): void {
		this.pictures.fire({ frame, role: 'changed', display: '7', revision: '3', decodedAt: performance.now() });
	}

	/** The other computer's pointer at `x`, `y` in the picture: a white 4 × 4 square, its hot spot at the top left. */
	public pointer(x: number, y: number, visible = true): void {
		const shape = { width: 4, height: 4, hotspotX: 0, hotspotY: 0, rgba: new Uint8ClampedArray(64).fill(255) };
		this.pointers.fire({ x, y, visible, shape });
	}

	public end(reason: DesktopEnd): void {
		this.ends.fire(reason);
	}

	public dispose(): void {
		this.disposed = true;
		this.pictures.dispose();
		this.ends.dispose();
		this.pointers.dispose();
	}
}

/**
 * An {@link IRemoteDesktopService} that opens sessions made by
 * `createSession`; a failure queued in `failures` answers the
 * next open instead. Recordings start in `runtime`.
 */
export class TestRemoteDesktopService implements Pick<IRemoteDesktopService, '_serviceBrand' | 'open' | 'startRecording'> {
	declare public readonly _serviceBrand: undefined;
	public readonly failures: DesktopFailure[] = [];
	private readonly opened = new Emitter<IDesktopSession>();
	/** Fires with each session opened. */
	public readonly onDidOpen = this.opened.event;

	constructor(private readonly runtime: TestRecordingRuntime, private readonly createSession: (control: boolean) => IDesktopSession) { }

	public open(_deviceId: string, control: boolean): Promise<{ readonly failure: DesktopFailure } | { readonly session: IDesktopSession }> {
		const failure = this.failures.shift();
		if (failure) {
			return Promise.resolve({ failure });
		}
		const session = this.createSession(control);
		this.opened.fire(session);
		return Promise.resolve({ session });
	}

	public startRecording(): Promise<{ readonly recording: string }> {
		return Promise.resolve({ recording: this.runtime.begin().id });
	}

	public dispose(): void {
		this.opened.dispose();
	}
}

/** A menu the view opened: from where, and what it offered. */
export interface ShownMenu {
	readonly anchor: unknown;
	readonly actions: readonly IAction[];
}

/** An {@link IContextMenuService} that keeps each menu shown and closes it at once. */
export class TestContextMenuService implements IContextMenuService {
	declare public readonly _serviceBrand: undefined;
	public readonly menus: ShownMenu[] = [];

	public showContextMenu(delegate: IContextMenuDelegate | IContextMenuMenuDelegate): void {
		this.menus.push({ anchor: delegate.getAnchor(), actions: delegate.getActions?.() ?? [] });
		delegate.onHide?.(false);
	}

	/** The labels of the last menu shown, separators as `-`. */
	public lastMenu(): string[] {
		return (this.menus.at(-1)?.actions ?? []).map(action => action.id === 'vs.actions.separator' ? '-' : action.label);
	}
}

/**
 * System actions where privacy mode follows each request, kept in
 * `privacyRequests`; `privacyError` refuses the next request with these words.
 */
export class TestDesktopSystemActionService extends TestSystemActionService {
	public readonly privacyRequests: boolean[] = [];
	public privacyError: string | undefined;

	public override privacy(): Promise<SystemPrivacyResult> {
		return Promise.resolve(this.privacyResult(this.privacyRequests.at(-1) ?? false));
	}

	public override setPrivacy(_deviceId: string, enabled: boolean): Promise<SystemPrivacyResult> {
		const error = this.privacyError;
		this.privacyError = undefined;
		if (error) {
			return Promise.resolve({ error });
		}
		this.privacyRequests.push(enabled);
		return Promise.resolve(this.privacyResult(enabled));
	}

	private privacyResult(enabled: boolean): SystemPrivacyResult {
		return { privacy: { displayPrivate: enabled, inputBlocked: enabled } };
	}
}

/** An {@link IUIStateService} held in `values`. */
export class TestUIStateService implements IUIStateService {
	declare public readonly _serviceBrand: undefined;
	public readonly values = new Map<UIStateKey, unknown>();

	public get<K extends UIStateKey>(key: K): Promise<UIStateValues[K] | undefined> {
		return Promise.resolve(this.values.get(key) as UIStateValues[K] | undefined);
	}

	public set<K extends UIStateKey>(key: K, value: UIStateValues[K] | undefined): Promise<void> {
		this.values.set(key, value);
		return Promise.resolve();
	}
}

/** The recordings of the native runtime: each started one, the status it reports, and which were stopped and released. */
export class TestRecordingRuntime implements Pick<INativeRuntimeService, '_serviceBrand' | 'startRecording' | 'recordingStatus' | 'stopRecording' | 'releaseRecording'> {
	declare public readonly _serviceBrand: undefined;
	/** The status each recording reports, which a test moves on. */
	public readonly recordings = new Map<string, RecordingStatus>();
	public readonly stopped: string[] = [];
	public readonly released: string[] = [];
	private next = 0;

	/** Starts a recording as the engine would. */
	public begin(): RecordingStatus {
		const id = String(++this.next);
		const status: RecordingStatus = { id, phase: 'Recording', failure: null, frames: '0', bytes: '0', durationMs: '0', segments: 1 };
		this.recordings.set(id, status);
		return status;
	}

	public startRecording(): Promise<RecordingStatus> {
		return Promise.resolve(this.begin());
	}

	public recordingStatus(id: string): Promise<RecordingStatus> {
		return this.status(id);
	}

	public stopRecording(id: string): Promise<RecordingStatus> {
		this.stopped.push(id);
		const status = this.recordings.get(id);
		if (status) {
			this.recordings.set(id, { ...status, phase: 'Finished' });
		}
		return this.status(id);
	}

	public releaseRecording(id: string): Promise<void> {
		this.released.push(id);
		return Promise.resolve();
	}

	private status(id: string): Promise<RecordingStatus> {
		const status = this.recordings.get(id);
		return status ? Promise.resolve(status) : Promise.reject(new Error(`No recording ${id}`));
	}
}

/** The window as a remote desktop tab uses it: full screen, and whether the picture has the keyboard. */
export class TestDesktopHostService extends Disposable implements Pick<INativeHostService, '_serviceBrand' | 'onDidChangeWindowState' | 'setFullScreen' | 'setRemoteKeyboard'> {
	declare public readonly _serviceBrand: undefined;
	private readonly windowState = this._register(new Emitter<IWindowState>());
	public readonly onDidChangeWindowState = this.windowState.event;
	public readonly fullScreen: boolean[] = [];
	/** Whether the picture had the keyboard, as each change told the window. */
	public readonly remoteKeyboard: boolean[] = [];

	public setFullScreen(value: boolean): Promise<void> {
		this.fullScreen.push(value);
		return Promise.resolve();
	}

	public setRemoteKeyboard(owned: boolean): Promise<void> {
		this.remoteKeyboard.push(owned);
		return Promise.resolve();
	}

	/** The window left full screen by itself, as from its own button. */
	public leaveFullScreen(): void {
		this.windowState.fire({ windowId: 1, revision: 1, focused: true, maximized: false, fullscreen: false, alwaysOnTop: false, zoomFactor: 1, backgroundMaterial: 'none', accentColor: undefined });
	}
}

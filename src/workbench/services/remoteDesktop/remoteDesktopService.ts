import * as nls from '../../../nls.js';
import type { Event } from '../../../base/common/event.js';
import type { IDisposable } from '../../../base/common/lifecycle.js';
import type { DesktopMediaStatus } from '../../../platform/engine/common/dataChannel.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { DisplayInfo, QualityLimits, VideoCodecCaps } from '../../../platform/engine/common/engine.js';
import type { DesktopLatencySummary } from './desktopLatency.js';

export interface DesktopTarget { readonly id: string; readonly generation: string; readonly role: 'Controller' | 'Host'; }

export interface DesktopSession extends DesktopTarget { readonly name: string; readonly quality: Readonly<QualityLimits>; }

export interface DesktopSettings { readonly sessions: readonly DesktopSession[]; readonly codecs: Readonly<VideoCodecCaps>; }

export function desktopTargetKey(target: DesktopTarget): string {
	return `${target.role}:${target.id}:${target.generation}`;
}

export const qualityPresets = [
	{ id: 'bandwidth', label: nls.localize('desktop.preset.bandwidth', 'Low bandwidth'), quality: { max_bitrate_kbps: 2000, max_frame_rate: 24, max_width: 1280, max_height: 720 } },
	{ id: 'balanced', label: nls.localize('desktop.preset.balanced', 'Balanced'), quality: { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 } },
	{ id: 'smooth', label: nls.localize('desktop.preset.smooth', 'Smooth'), quality: { max_bitrate_kbps: 12000, max_frame_rate: 60, max_width: 1920, max_height: 1080 } },
] as const;

export function isValidDesktopQuality(quality: QualityLimits): boolean {
	return Number.isInteger(quality.max_bitrate_kbps) && quality.max_bitrate_kbps >= 1 && quality.max_bitrate_kbps <= 50000 &&
		Number.isInteger(quality.max_frame_rate) && quality.max_frame_rate >= 1 && quality.max_frame_rate <= 60 &&
		[quality.max_width, quality.max_height].every(value => Number.isInteger(value) && value >= 1 && value <= 7680);
}

/** Why a paired computer's screen can't be opened. */
export type DesktopFailure =
	/** This computer isn't connected to it. */
	| 'disconnected'
	/** Its user doesn't share the screen, or its control, with this computer. */
	| 'turnedOff'
	/** Its system hasn't allowed VniLink to record the screen or control it. */
	| 'needsPermission'
	/** It can't share its screen. */
	| 'unsupported'
	/** It can't open another service right now. */
	| 'unavailable'
	/** Its screen didn't start. */
	| 'failed';

/** Why a running session stopped: the other computer's word when it gave one, otherwise what the engine saw. */
export type DesktopEnd =
	/** The connection was lost, or this window's channel to the engine stopped. */
	| 'linkLost'
	/** The connection was lost while it went through a relay. */
	| 'relayUnreachable'
	/** The other computer finished the stream without saying why. */
	| 'peerClosed'
	/** Its encoder failed. */
	| 'hostEncoderFailed'
	/** Its screen capture stopped. */
	| 'hostCaptureFailed'
	/** Its system withdrew VniLink's permission to record the screen. */
	| 'hostPermission'
	/** The display being shown went away. */
	| 'displayGone'
	/** Its user stopped sharing, or closed the connection. */
	| 'hostStopped'
	/** VniLink quit on it. */
	| 'hostShutdown'
	/** Its user withdrew this computer's permission. */
	| 'revoked'
	/** This computer can't decode the picture. */
	| 'cantShow';

/** The ends the engine reports in a picture channel's last packet. */
export const engineDesktopEnds: readonly DesktopEnd[] = ['linkLost', 'relayUnreachable', 'peerClosed', 'hostEncoderFailed', 'hostCaptureFailed', 'hostPermission', 'displayGone', 'hostStopped', 'hostShutdown', 'revoked'];

/** One picture of a display on the other computer, at the topology revision it was taken with. */
export interface DesktopPicture {
	/** Belongs to the listener, which closes it once drawn. */
	readonly frame: VideoFrame;
	readonly display: string;
	readonly revision: string;
	/** When it came out of the decoder, on `performance.now()`. */
	readonly decodedAt: number;
}

/** The other computer's pointer, where it is on the shown picture, in its pixels. */
export interface DesktopPointer {
	readonly x: number;
	readonly y: number;
	readonly visible: boolean;
	/** Its current shape; `undefined` until the first one arrives. */
	readonly shape: DesktopPointerShape | undefined;
}

/** A pointer shape in the picture's pixels: straight-alpha RGBA, and the point under the mouse. */
export interface DesktopPointerShape {
	readonly width: number;
	readonly height: number;
	readonly hotspotX: number;
	readonly hotspotY: number;
	readonly rgba: Uint8ClampedArray<ArrayBuffer>;
}

/** Input for the other computer, as the engine's input commands carry it without their session. */
export type DesktopInput =
	| { readonly type: 'submitLocalKey'; readonly usage: number; readonly down: boolean; readonly text: string; readonly composing: boolean }
	| { readonly type: 'submitPointer'; readonly x: number; readonly y: number; readonly display: string; readonly revision: string }
	| { readonly type: 'submitButton'; readonly button: 'Left' | 'Right' | 'Middle' | 'Back' | 'Forward'; readonly down: boolean }
	| { readonly type: 'submitPreciseWheel'; readonly dx: number; readonly dy: number }
	| { readonly type: 'releaseSessionInput' }
	/** The display the pointer lands on; the other computer takes the pointer only there. */
	| { readonly type: 'selectDisplays'; readonly ids: string[] };

/** How the picture is doing: the engine's view of it, and the codec it arrives in. */
export interface DesktopStats {
	readonly status: DesktopMediaStatus;
	/** `h264`, `h265`, `vp8`, `vp9` or `av1`. */
	readonly codec: string;
	/** Where the pictures' time went since the last read. */
	readonly latency: DesktopLatencySummary;
	/** Pictures replaced by newer ones before they were drawn, since the last read. */
	readonly skipped: number;
}

/** A running view, or control, of a paired computer's screen. */
export interface IDesktopSession extends IDisposable {
	/** Whether input reaches the other computer; a view-only session sends none. */
	readonly control: boolean;
	readonly onPicture: Event<DesktopPicture>;
	/**
	 * The other computer's pointer moved or changed shape. Only when it is
	 * left out of the picture, for this computer to draw.
	 */
	readonly onPointer: Event<DesktopPointer>;
	/** Fires once; disposing the session first fires nothing. */
	readonly onEnd: Event<DesktopEnd>;
	/** The other computer's displays, where its pictures and pointer positions lie. */
	displays(): Promise<readonly DisplayInfo[]>;
	/** Sends input in order. Ignored in a view-only session or once ended. */
	send(input: DesktopInput): void;
	/** The picture is on screen, which ends its measured time. */
	presented(picture: DesktopPicture): void;
	/** A newer picture replaced this one before it was drawn. */
	skipped(picture: DesktopPicture): void;
	/** How the picture is doing now; `undefined` once ended or before the first picture. */
	stats(): Promise<DesktopStats | undefined>;
}

export const IRemoteDesktopService = createDecorator<IRemoteDesktopService>('remoteDesktopService');

export interface IRemoteDesktopService {
	readonly _serviceBrand: undefined;
	readonly available: boolean;
	load(): Promise<DesktopSettings>;
	displays(target: DesktopTarget): Promise<readonly DisplayInfo[]>;
	apply(target: DesktopTarget, quality: QualityLimits): Promise<DesktopSettings>;
	/** Opens the screen of `deviceId`; with `control`, its keyboard and pointer too. */
	open(deviceId: string, control: boolean): Promise<{ readonly failure: DesktopFailure } | { readonly session: IDesktopSession }>;
}

export class DesktopReadbackError extends Error {
	constructor() {
		super('Quality update accepted, but current session could not be reloaded');
	}
}

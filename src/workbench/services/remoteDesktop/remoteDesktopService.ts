import * as nls from '../../../nls.js';
import type { Event } from '../../../base/common/event.js';
import type { IDisposable } from '../../../base/common/lifecycle.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { DisplayInfo, QualityLimits, VideoCodecCaps } from '../../../platform/engine/common/engine.js';

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

/** Why a running session stopped. */
export type DesktopEnd =
	/** The connection ended, or the other computer stopped sending its screen. */
	| 'lost'
	/** This computer can't decode the picture. */
	| 'cantShow';

/** One picture of a display on the other computer, at the topology revision it was taken with. */
export interface DesktopPicture {
	/** Belongs to the listener, which closes it once drawn. */
	readonly frame: VideoFrame;
	readonly display: string;
	readonly revision: string;
}

/** Input for the other computer, as the engine's input commands carry it without their session. */
export type DesktopInput =
	| { readonly type: 'submitLocalKey'; readonly usage: number; readonly down: boolean; readonly text: string }
	| { readonly type: 'submitPointer'; readonly x: number; readonly y: number; readonly display: string; readonly revision: string }
	| { readonly type: 'submitButton'; readonly button: 'Left' | 'Right' | 'Middle' | 'Back' | 'Forward'; readonly down: boolean }
	| { readonly type: 'submitPreciseWheel'; readonly dx: number; readonly dy: number }
	| { readonly type: 'releaseSessionInput' };

/** A running view, or control, of a paired computer's screen. */
export interface IDesktopSession extends IDisposable {
	/** Whether input reaches the other computer; a view-only session sends none. */
	readonly control: boolean;
	readonly onPicture: Event<DesktopPicture>;
	/** Fires once; disposing the session first fires nothing. */
	readonly onEnd: Event<DesktopEnd>;
	/** The other computer's displays, where its pictures and pointer positions lie. */
	displays(): Promise<readonly DisplayInfo[]>;
	/** Sends input in order. Ignored in a view-only session or once ended. */
	send(input: DesktopInput): void;
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

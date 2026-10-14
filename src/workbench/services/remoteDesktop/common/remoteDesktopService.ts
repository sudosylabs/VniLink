import { createDecorator } from '../../../../platform/instantiation/common/instantiation.js';
import type { DisplayInfo, QualityLimits, VideoCodecCaps } from '../../../../platform/engine/common/engine.js';
export interface DesktopTarget { readonly id: string; readonly generation: string; readonly role: 'Controller' | 'Host'; }
export interface DesktopSession extends DesktopTarget { readonly name: string; readonly quality: Readonly<QualityLimits>; }
export interface DesktopSettings { readonly sessions: readonly DesktopSession[]; readonly codecs: Readonly<VideoCodecCaps>; }
export function desktopTargetKey(target: DesktopTarget): string {
	return `${target.role}:${target.id}:${target.generation}`;
}
export const qualityPresets = [
	{ id: 'bandwidth', label: 'Low bandwidth', quality: { max_bitrate_kbps: 2000, max_frame_rate: 24, max_width: 1280, max_height: 720 } },
	{ id: 'balanced', label: 'Balanced', quality: { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 } },
	{ id: 'smooth', label: 'Smooth', quality: { max_bitrate_kbps: 12000, max_frame_rate: 60, max_width: 1920, max_height: 1080 } },
] as const;
export function isValidDesktopQuality(quality: QualityLimits): boolean {
	return Number.isInteger(quality.max_bitrate_kbps) && quality.max_bitrate_kbps >= 1 && quality.max_bitrate_kbps <= 50000 &&
		Number.isInteger(quality.max_frame_rate) && quality.max_frame_rate >= 1 && quality.max_frame_rate <= 60 &&
		[quality.max_width, quality.max_height].every(value => Number.isInteger(value) && value >= 1 && value <= 7680);
}
export const IRemoteDesktopService = createDecorator<IRemoteDesktopService>('remoteDesktopService');
export interface IRemoteDesktopService {
	readonly _serviceBrand: undefined;
	readonly available: boolean;
	load(): Promise<DesktopSettings>;
	displays(target: DesktopTarget): Promise<readonly DisplayInfo[]>;
	apply(target: DesktopTarget, quality: QualityLimits): Promise<DesktopSettings>;
}
export class DesktopReadbackError extends Error {
	constructor() {
		super('Quality update accepted, but current session could not be reloaded');
	}
}

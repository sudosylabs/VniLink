import { record } from './engine.js';

export type DataChannelKind = 'video' | 'terminal' | 'input' | 'files' | 'camera' | 'audio' | 'snapshot';

export type MediaChannelConfig = { readonly type: 'camera'; readonly deviceId: string; readonly width: number; readonly height: number; readonly fps: number } | { readonly type: 'audio'; readonly deviceId: string; readonly sourceKind: 'Microphone' | 'System'; readonly channels: 1 | 2; readonly voice: boolean } | { readonly type: 'snapshot'; readonly prompt: boolean; readonly access: boolean };

export interface DataChannelRequest {
	media?: MediaChannelConfig;
	requestId: string;
	kind: DataChannelKind;
	sessionId: string;
	generation: string;
	rows?: number;
	columns?: number;
}

export interface DataPacket {
	type: 'data';
	sequence: number;
	kind: string;
	data?: Uint8Array;
	metadata: string;
	/** When the engine's process read the packet, on the shared high-resolution clock (`performance.timeOrigin + performance.now()`). */
	readAt?: number;
}

export interface DataChannelMetrics {
	sent: number;
	acknowledged: number;
	pending: number;
	lastAckMs: number;
}

export function isChannelRequest(value: unknown): value is DataChannelRequest {
	return (
		record(value) &&
		typeof value.requestId === 'string' &&
		/^[a-zA-Z0-9-]{1,64}$/.test(value.requestId) &&
		typeof value.kind === 'string' && ['video', 'terminal', 'input', 'files', 'camera', 'audio', 'snapshot'].includes(value.kind) &&
		id(value.sessionId) &&
		id(value.generation) &&
		(value.kind !== 'terminal' || (dimension(value.rows) && dimension(value.columns))) &&
		(value.kind === 'snapshot' ? isSnapshotMedia(value.media) : (value.kind === 'camera' || value.kind === 'audio') ? isMediaChannelConfig(value.media) && value.media.type === value.kind : value.media === undefined) &&
		Object.keys(value).every((key) =>
			['requestId', 'kind', 'sessionId', 'generation', 'rows', 'columns', 'media'].includes(key),
		)
	);
}

export function id(value: unknown): value is string {
	return typeof value === 'string' && /^[1-9]\d{0,19}$/.test(value) && BigInt(value) <= 18446744073709551615n;
}

export function dimension(value: unknown): value is number {
	return typeof value === 'number' && Number.isInteger(value) && value > 0 && value <= 65535;
}

export function isDataPacket(value: unknown): value is DataPacket {
	return (
		record(value) &&
		value.type === 'data' &&
		Number.isSafeInteger(value.sequence) &&
		Number(value.sequence) > 0 &&
		typeof value.kind === 'string' &&
		typeof value.metadata === 'string' &&
		value.metadata.length <= 16384 &&
		(value.data === undefined || (value.data instanceof Uint8Array && value.data.byteLength <= 2 * 1024 * 1024)) &&
		(value.readAt === undefined || (typeof value.readAt === 'number' && Number.isFinite(value.readAt)))
	);
}

/** File operations on a `files` channel; see `files.ts`. */
export const MAX_FILE_OPERATIONS = 4;

export function isSnapshotMedia(value: unknown): value is { type: 'snapshot'; prompt: boolean; access: boolean } {
	return record(value) && Object.keys(value).length === 3 && value.type === 'snapshot' && typeof value.prompt === 'boolean' && typeof value.access === 'boolean';
}

export function isMediaChannelConfig(value: unknown): value is MediaChannelConfig {
	if (!record(value) || typeof value.deviceId !== 'string' || value.deviceId.length === 0 || value.deviceId.length > 256 || value.deviceId.includes('\0') || Object.keys(value).length !== 5) {
		return false;
	}
	if (value.type === 'camera') {
		return dimension(value.width) && value.width <= 4096 && value.width % 2 === 0
			&& dimension(value.height) && value.height <= 4096 && value.height % 2 === 0
			&& dimension(value.fps) && value.fps <= 60;
	}
	return value.type === 'audio' && (value.sourceKind === 'Microphone' || value.sourceKind === 'System') && (value.channels === 1 || value.channels === 2) && typeof value.voice === 'boolean';
}

export type DesktopAudioOperation = { type: 'status' } | { type: 'enabled'; enabled: boolean } | { type: 'muted'; muted: boolean };

export interface DesktopAudioStatus {
	phase: 'starting' | 'playing' | 'muted' | 'unavailable' | 'stopped';
	enabled: boolean;
	muted: boolean;
	error: { code: string; message: string } | null;
}

/** Something wrong with a live picture, as the engine names it; see `vnilink::Condition`. */
export type DesktopCondition =
	| 'link.blackout' | 'relay.backlog' | 'link.congestion' | 'link.loss' | 'host.secureDesktop'
	| 'encoder.slow' | 'decoder.slow' | 'decoder.recovering' | 'encoder.software' | 'decoder.software' | 'relay.path';

/** Every condition, from the most to the least severe: the engine's order. */
export const desktopConditions: readonly DesktopCondition[] = ['link.blackout', 'relay.backlog', 'link.congestion', 'link.loss', 'host.secureDesktop', 'encoder.slow', 'decoder.slow', 'decoder.recovering', 'encoder.software', 'decoder.software', 'relay.path'];

/** What most holds a picture's bitrate where it is, as the engine names it; see `vnilink::RateLimit`. */
export type DesktopRateLimit = 'none' | 'setting' | 'relay' | 'network' | 'encoder' | 'receiver';

/** Every rate limit, as the engine names it. */
export const desktopRateLimits: readonly DesktopRateLimit[] = ['none', 'setting', 'relay', 'network', 'encoder', 'receiver'];

/** How a desktop picture is doing, from the engine's adaptation: what arrives, what waits, why the quality is where it is, and what is wrong. */
export interface DesktopMediaStatus {
	reason: 'warmingUp' | 'stable' | 'receiverPressure' | 'encoderPressure' | 'networkPressure' | 'recovering' | 'idle';
	receivedKbps: number;
	receivedFps: number;
	receiverQueueMs: number;
	hostQueueMs: number;
	lossPerMille: number;
	rttMs: number;
	encodeUs: number;
	captureUs: number;
	/** What holds the bitrate where it is; `none` when nothing does. */
	limit: DesktopRateLimit;
	/** Most severe first; empty when nothing is wrong. */
	conditions: DesktopCondition[];
}

/** Held conditions: known, each once, in the engine's order. */
function conditions(value: unknown): value is DesktopCondition[] {
	if (!Array.isArray(value)) {
		return false;
	}
	const ranks = value.map((condition: unknown) => desktopConditions.indexOf(condition as DesktopCondition));
	return ranks.every((rank, index) => rank >= 0 && (index === 0 || rank > ranks[index - 1]));
}

export function isDesktopMediaStatus(value: unknown): value is DesktopMediaStatus {
	const counts = ['receivedKbps', 'receivedFps', 'receiverQueueMs', 'hostQueueMs', 'lossPerMille', 'rttMs', 'encodeUs', 'captureUs'] as const;
	return record(value) && Object.keys(value).length === 11
		&& ['warmingUp', 'stable', 'receiverPressure', 'encoderPressure', 'networkPressure', 'recovering', 'idle'].includes(String(value.reason))
		&& counts.every(key => Number.isSafeInteger(value[key]) && Number(value[key]) >= 0)
		&& desktopRateLimits.includes(value.limit as DesktopRateLimit)
		&& conditions(value.conditions);
}

/** What the window's decoder says of itself with each status read. */
export interface DesktopDecoderReport {
	/** Its time per picture while others wait for it, in microseconds on average; 0 when unknown. */
	readonly decodeUs: number;
	/** It fell back from a hardware decoder. */
	readonly software: boolean;
	/** Times it restarted from a keyframe after an error, in this session. */
	readonly recoveries: number;
	/** Still pictures shown, in this session. */
	readonly stillsShown: number;
}

/** The other computer's pointer, in the pixels of the picture it is shown on. */
export interface DesktopCursor {
	readonly x: number;
	readonly y: number;
	readonly visible: boolean;
	/** Only when the shape changed: premultiplied BGRA, and the point under the mouse. */
	readonly image?: DesktopCursorImage;
}

export interface DesktopCursorImage {
	readonly width: number;
	readonly height: number;
	readonly hotspotX: number;
	readonly hotspotY: number;
	readonly pixels: Uint8Array;
}

/** The largest pointer side the engine carries. */
export const MAX_CURSOR_EDGE = 512;

function isCursorImage(value: unknown): value is DesktopCursorImage {
	if (!record(value) || Object.keys(value).length !== 5 || !(value.pixels instanceof Uint8Array)) {
		return false;
	}
	const { width, height, hotspotX, hotspotY } = value;
	return Number.isInteger(width) && Number.isInteger(height) && Number(width) >= 1 && Number(height) >= 1
		&& Number(width) <= MAX_CURSOR_EDGE && Number(height) <= MAX_CURSOR_EDGE
		&& Number.isInteger(hotspotX) && Number.isInteger(hotspotY) && Number(hotspotX) >= 0 && Number(hotspotY) >= 0
		&& Number(hotspotX) < Number(width) && Number(hotspotY) < Number(height)
		&& value.pixels.length === Number(width) * Number(height) * 4;
}

export function isDesktopCursor(value: unknown): value is DesktopCursor {
	if (!record(value) || typeof value.visible !== 'boolean' || !Number.isSafeInteger(value.x) || !Number.isSafeInteger(value.y)) {
		return false;
	}
	return 'image' in value
		? Object.keys(value).length === 4 && isCursorImage(value.image)
		: Object.keys(value).length === 3;
}

export function isDesktopAudioOperation(value: unknown): value is DesktopAudioOperation {
	return record(value) && ((value.type === 'status' && Object.keys(value).length === 1)
		|| (value.type === 'enabled' && typeof value.enabled === 'boolean' && Object.keys(value).length === 2)
		|| (value.type === 'muted' && typeof value.muted === 'boolean' && Object.keys(value).length === 2));
}

export function isDesktopAudioStatus(value: unknown): value is DesktopAudioStatus {
	return record(value) && Object.keys(value).length === 4 && typeof value.phase === 'string'
		&& ['starting', 'playing', 'muted', 'unavailable', 'stopped'].includes(value.phase)
		&& typeof value.enabled === 'boolean' && typeof value.muted === 'boolean'
		&& (value.error === null || (record(value.error) && Object.keys(value.error).length === 2 && typeof value.error.code === 'string' && value.error.code.length > 0 && value.error.code.length <= 64 && typeof value.error.message === 'string' && value.error.message.length <= 2048));
}

import { isEngineCommand, record } from './engine.js';
import type { EngineCommand } from './engine.js';

export type DataChannelKind = 'video' | 'terminal' | 'input' | 'files' | 'camera' | 'audio';

export type MediaChannelConfig = { readonly type: 'camera'; readonly deviceId: string; readonly width: number; readonly height: number; readonly fps: number } | { readonly type: 'audio'; readonly deviceId: string; readonly sourceKind: 'Microphone' | 'System'; readonly channels: 1 | 2; readonly voice: boolean };

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
		typeof value.kind === 'string' && ['video', 'terminal', 'input', 'files', 'camera', 'audio'].includes(value.kind) &&
		id(value.sessionId) &&
		id(value.generation) &&
		(value.kind !== 'terminal' || (dimension(value.rows) && dimension(value.columns))) &&
		((value.kind === 'camera' || value.kind === 'audio') ? isMediaChannelConfig(value.media) && value.media.type === value.kind : value.media === undefined) &&
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

export function isInputCommand(value: unknown): value is EngineCommand {
	return (
		isEngineCommand(value) &&
		[
			'submitKey',
			'submitPointer',
			'submitButton',
			'submitWheel',
			'submitMotion',
			'submitPreciseWheel',
			'submitText',
			'submitLocalKey',
			'releaseSessionInput',
		].includes(value.type)
	);
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
		(value.data === undefined || (value.data instanceof Uint8Array && value.data.byteLength <= 2 * 1024 * 1024))
	);
}

export type FileOperation =
	| {
		type: 'submit';
		direction: 'upload' | 'download';
		srcRoot: string;
		srcPath: string;
		dstRoot: string;
		dstPath: string;
	}
	| { type: 'poll' | 'cancel'; jobId: string };

export function isMediaChannelConfig(value: unknown): value is MediaChannelConfig {
	if (!record(value) || typeof value.deviceId !== 'string' || value.deviceId.length === 0 || value.deviceId.length > 256 || value.deviceId.includes('\0') || Object.keys(value).length !== 5) {
		return false;
	}
	if (value.type === 'camera') {
		return dimension(value.width) && value.width <= 4096 && value.width % 2 === 0 && dimension(value.height) && value.height <= 4096 && value.height % 2 === 0 && dimension(value.fps) && value.fps <= 60;
	}
	return value.type === 'audio' && (value.sourceKind === 'Microphone' || value.sourceKind === 'System') && (value.channels === 1 || value.channels === 2) && typeof value.voice === 'boolean';
}

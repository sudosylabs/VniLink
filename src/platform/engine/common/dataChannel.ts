import { isEngineCommand, record } from './engine.js';
import type { EngineCommand } from './engine.js';

export type DataChannelKind = 'video' | 'terminal' | 'input' | 'files';
export interface DataChannelRequest {
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
		typeof value.kind === 'string' && ['video', 'terminal', 'input', 'files'].includes(value.kind) &&
		id(value.sessionId) &&
		id(value.generation) &&
		(value.kind !== 'terminal' || (dimension(value.rows) && dimension(value.columns))) &&
		Object.keys(value).every((key) =>
			['requestId', 'kind', 'sessionId', 'generation', 'rows', 'columns'].includes(key),
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

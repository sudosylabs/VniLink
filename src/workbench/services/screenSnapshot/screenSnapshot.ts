import { id } from '../../../platform/engine/common/dataChannel.js';
import { record } from '../../../platform/engine/common/engine.js';

const MAX_SNAPSHOT_BYTES = 512 * 1024;

const MAX_SNAPSHOT_WIDTH = 640;

const MAX_SNAPSHOT_HEIGHT = 360;

export type ScreenSnapshotFailure = 'needsPermission' | 'needsSourceSelection' | 'unavailable' | 'busy' | 'timedOut' | 'sharingDisabled' | 'disconnected' | 'replaced' | 'unpaired' | 'unsupported' | 'failed' | 'cancelled';

export type ScreenSnapshotResult =
	| {
		readonly status: 'ready';
		readonly jpeg: Uint8Array;
		readonly width: number;
		readonly height: number;
		readonly capturedUnixMs: number;
		readonly displayId: string;
		readonly endpointId: string;
		readonly sessionId: string;
		readonly generation: string;
	}
	| { readonly status: ScreenSnapshotFailure };

export type ScreenAccess =
	| { readonly status: 'allowed'; readonly endpointId: string; readonly sessionId: string; readonly generation: string }
	| { readonly status: 'denied' | 'unavailable' | 'disconnected' | 'replaced' | 'unpaired' | 'unsupported' | 'timedOut' | 'busy' | 'failed' | 'cancelled' };

const FAILURES: readonly ScreenSnapshotFailure[] = ['needsPermission', 'needsSourceSelection', 'unavailable', 'busy', 'timedOut', 'sharingDisabled', 'disconnected', 'replaced', 'unpaired', 'unsupported', 'failed', 'cancelled'];

const ACCESS_OUTCOMES = ['allowed', 'sharingDisabled', 'unavailable', 'disconnected', 'replaced', 'unpaired', 'unsupported', 'timedOut', 'busy', 'failed', 'cancelled'] as const;

export type ScreenAccessOutcome = typeof ACCESS_OUTCOMES[number];

export function parseScreenSnapshot(metadata: string, data: Uint8Array | undefined): ScreenSnapshotResult | undefined {
	const parsed = outcome(metadata);
	if (!parsed) {
		return undefined;
	}
	if (parsed.outcome === 'ready') {
		return parseReady(parsed, data);
	}
	if (FAILURES.includes(parsed.outcome as ScreenSnapshotFailure) && Object.keys(parsed).length === 1 && data === undefined) {
		return { status: parsed.outcome as ScreenSnapshotFailure };
	}
	return undefined;
}

export function parseScreenAccess(metadata: string, data: Uint8Array | undefined): ScreenAccessOutcome | undefined {
	const parsed = outcome(metadata);
	if (!parsed || data !== undefined || Object.keys(parsed).length !== 1) {
		return undefined;
	}
	return ACCESS_OUTCOMES.find(item => item === parsed.outcome);
}

function outcome(metadata: string): Record<string, unknown> | undefined {
	let parsed: unknown;
	try {
		parsed = JSON.parse(metadata);
	} catch {
		return undefined;
	}
	if (!record(parsed) || typeof parsed.outcome !== 'string') {
		return undefined;
	}
	return parsed;
}

function parseReady(parsed: Record<string, unknown>, data: Uint8Array | undefined): ScreenSnapshotResult | undefined {
	if (Object.keys(parsed).length !== 9 || parsed.mime !== 'image/jpeg' || typeof parsed.displayId !== 'string' || parsed.displayId.length === 0 || parsed.displayId.length > 64) {
		return undefined;
	}
	if (!integer(parsed.width, MAX_SNAPSHOT_WIDTH) || !integer(parsed.height, MAX_SNAPSHOT_HEIGHT) || !endpoint(parsed.endpointId) || !id(parsed.sessionId) || !id(parsed.generation)) {
		return undefined;
	}
	const capturedUnixMs = unixMilliseconds(parsed.capturedUnixMs);
	const size = data ? jpegSize(data) : undefined;
	if (capturedUnixMs === undefined || !data || !isJpeg(data) || size?.width !== parsed.width || size.height !== parsed.height) {
		return undefined;
	}
	return {
		status: 'ready',
		jpeg: data.slice(),
		width: parsed.width,
		height: parsed.height,
		capturedUnixMs,
		displayId: parsed.displayId,
		endpointId: parsed.endpointId,
		sessionId: parsed.sessionId,
		generation: parsed.generation,
	};
}

function endpoint(value: unknown): value is string {
	return typeof value === 'string' && value.length >= 1 && value.length <= 128 && /^[\x21-\x7e]+$/.test(value);
}

function integer(value: unknown, max: number): value is number {
	return typeof value === 'number' && Number.isInteger(value) && value >= 1 && value <= max;
}

function unixMilliseconds(value: unknown): number | undefined {
	if (typeof value !== 'string' || !/^\d{1,16}$/.test(value)) {
		return undefined;
	}
	const parsed = BigInt(value);
	if (parsed > BigInt(Number.MAX_SAFE_INTEGER)) {
		return undefined;
	}
	return Number(parsed);
}

function isJpeg(bytes: Uint8Array): boolean {
	return bytes.byteLength >= 4
		&& bytes.byteLength <= MAX_SNAPSHOT_BYTES
		&& bytes[0] === 0xff
		&& bytes[1] === 0xd8
		&& bytes[bytes.byteLength - 2] === 0xff
		&& bytes[bytes.byteLength - 1] === 0xd9;
}

/** SOF dimensions. This does not decode the entropy. */
export function jpegSize(jpeg: Uint8Array): { width: number; height: number } | undefined {
	if (!isJpeg(jpeg)) {
		return undefined;
	}
	let index = 2;
	while (index + 1 < jpeg.byteLength) {
		if (jpeg[index] !== 0xff) {
			return undefined;
		}
		while (index < jpeg.byteLength && jpeg[index] === 0xff) {
			index += 1;
		}
		if (index >= jpeg.byteLength) {
			return undefined;
		}
		const marker = jpeg[index];
		index += 1;
		if (marker === 0xd8 || marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) {
			continue;
		}
		if (marker === 0xd9 || marker === 0xda) {
			return undefined;
		}
		if (index + 1 >= jpeg.byteLength) {
			return undefined;
		}
		const length = (jpeg[index] << 8) | jpeg[index + 1];
		if (length < 2 || index + length > jpeg.byteLength) {
			return undefined;
		}
		if (marker === 0xc0 || marker === 0xc1 || marker === 0xc2) {
			if (length < 7) {
				return undefined;
			}
			const height = (jpeg[index + 3] << 8) | jpeg[index + 4];
			const width = (jpeg[index + 5] << 8) | jpeg[index + 6];
			return { width, height };
		}
		index += length;
	}
	return undefined;
}

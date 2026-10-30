// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { hardwareMatrix, type StreamMatrix } from './decoderColour.js';
import type { CameraChunk, CameraDecoder, CameraDecoderFactory } from './engineCameraStream.js';

/** A WebCodecs H.264 decoder for Annex-B pictures, hardware first. */
class WebCodecsDecoder implements CameraDecoder {
	private readonly waiting = new Set<() => void>();

	constructor(private readonly decoder: VideoDecoder, public readonly software: boolean) {
		decoder.addEventListener('dequeue', () => {
			this.settle();
		});
	}

	public decode(chunk: CameraChunk): void {
		if (this.decoder.state === 'configured') {
			const { data } = chunk;
			// Bytes that fill their own buffer, as a packet off the engine's
			// channel does, are handed to the decoder rather than copied.
			const transfer = data.byteOffset === 0 && data.byteLength === data.buffer.byteLength && data.buffer instanceof ArrayBuffer ? [data.buffer] : [];
			// Chromium takes `transfer`; this TypeScript's DOM types predate it.
			const init: EncodedVideoChunkInit & { readonly transfer: ArrayBuffer[] } = { type: chunk.key ? 'key' : 'delta', timestamp: chunk.timestamp, data, transfer };
			this.decoder.decode(new EncodedVideoChunk(init));
		}
	}

	public drained(limit: number): Promise<void> {
		if (this.isDrained(limit)) {
			return Promise.resolve();
		}
		return new Promise<void>(resolve => {
			const check = (): void => {
				if (this.isDrained(limit)) {
					this.waiting.delete(check);
					resolve();
				}
			};
			this.waiting.add(check);
		});
	}

	public close(): void {
		if (this.decoder.state !== 'closed') {
			this.decoder.close();
		}
		this.settle();
	}

	private isDrained(limit: number): boolean {
		return this.decoder.state !== 'configured' || this.decoder.decodeQueueSize <= limit;
	}

	private settle(): void {
		for (const check of [...this.waiting]) {
			check();
		}
	}
}

/**
 * The matrix this window decodes `config` in, or `undefined` where it does
 * not decode with it: WebCodecs' own answer, a refusal to answer being no,
 * and for a hardware decoder the matrix it shows correctly
 * ({@link hardwareMatrix}), none being no. A software decoder follows the
 * stream, so BT.709, the default.
 */
export async function webCodecsSupport(config: VideoDecoderConfig): Promise<StreamMatrix | undefined> {
	try {
		if ((await VideoDecoder.isConfigSupported(config)).supported !== true) {
			return undefined;
		}
	} catch {
		return undefined;
	}
	return config.hardwareAcceleration === 'prefer-hardware' ? hardwareMatrix(config) : 'bt709';
}

/**
 * Annex-B input carries its parameter sets in band, so no `description` is
 * given. Hardware first, then software, never `no-preference`, which may pick
 * the hardware decoder {@link webCodecsSupport} turned down.
 */
export const createWebCodecsDecoder: CameraDecoderFactory = async (codec, output, error) => {
	for (const hardwareAcceleration of ['prefer-hardware', 'prefer-software'] as const) {
		const config: VideoDecoderConfig = { codec, hardwareAcceleration, optimizeForLatency: true };
		if (await webCodecsSupport(config) === undefined) {
			continue;
		}
		const decoder = new VideoDecoder({ output, error });
		decoder.configure(config);
		return new WebCodecsDecoder(decoder, hardwareAcceleration === 'prefer-software');
	}
	return undefined;
};

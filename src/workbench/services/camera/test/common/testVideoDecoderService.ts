// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { StreamMatrix } from '../../decoderColour.js';
import type { CameraChunk, CameraDecoder } from '../../engineCameraStream.js';
import type { IVideoDecoderService } from '../../videoDecoderService.js';

/**
 * A decoder that keeps every chunk it is given in its queue until a test
 * calls `dequeue`, as a decoder that falls behind does. A test shows a
 * picture with `output` and fails the decoder with `error`.
 */
export class TestVideoDecoder implements CameraDecoder {
	public readonly software = false;
	public readonly chunks: CameraChunk[] = [];
	public closed = false;
	private queue = 0;
	private readonly waiting = new Set<() => boolean>();

	constructor(
		public readonly codec: string,
		public readonly output: (frame: VideoFrame) => void,
		public readonly error: () => void,
	) { }

	public decode(chunk: CameraChunk): void {
		this.chunks.push(chunk);
		this.queue++;
	}

	public drained(limit: number): Promise<void> {
		if (this.queue <= limit) {
			return Promise.resolve();
		}
		return new Promise(resolve => {
			const check = (): boolean => {
				if (this.queue > limit) {
					return false;
				}
				this.waiting.delete(check);
				resolve();
				return true;
			};
			this.waiting.add(check);
		});
	}

	/** One picture leaves the queue. */
	public dequeue(): void {
		this.queue--;
		for (const check of [...this.waiting]) {
			check();
		}
	}

	public close(): void {
		this.closed = true;
	}
}

/**
 * An {@link IVideoDecoderService} whose decoders a test drives. Each decoder
 * it created is kept in `decoders`; with `decodes` false it creates none, as
 * a window without a decoder for the codec. `support` answers with
 * `supportOf`, and every configuration asked about is kept in `probed`.
 */
export class TestVideoDecoderService implements IVideoDecoderService {
	declare public readonly _serviceBrand: undefined;
	public readonly decoders: TestVideoDecoder[] = [];
	public readonly probed: VideoDecoderConfig[] = [];
	public decodes = true;
	public supportOf: (config: VideoDecoderConfig) => StreamMatrix | undefined = () => undefined;

	public createDecoder(codec: string, output: (frame: VideoFrame) => void, error: () => void): Promise<CameraDecoder | undefined> {
		if (!this.decodes) {
			return Promise.resolve(undefined);
		}
		const decoder = new TestVideoDecoder(codec, output, error);
		this.decoders.push(decoder);
		return Promise.resolve(decoder);
	}

	public support(config: VideoDecoderConfig): Promise<StreamMatrix | undefined> {
		this.probed.push(config);
		return Promise.resolve(this.supportOf(config));
	}
}

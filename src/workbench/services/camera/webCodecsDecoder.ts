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

/** Annex-B input carries its parameter sets in band, so no `description` is given. */
export const createWebCodecsDecoder: CameraDecoderFactory = async (codec, output, error) => {
	for (const hardwareAcceleration of ['prefer-hardware', 'no-preference'] as const) {
		const config: VideoDecoderConfig = { codec, hardwareAcceleration, optimizeForLatency: true };
		try {
			if (!(await VideoDecoder.isConfigSupported(config)).supported) {
				continue;
			}
		} catch {
			continue;
		}
		const decoder = new VideoDecoder({ output, error });
		decoder.configure(config);
		return new WebCodecsDecoder(decoder, hardwareAcceleration === 'no-preference');
	}
	return undefined;
};

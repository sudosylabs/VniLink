import type { CameraChunk, CameraDecoder, CameraDecoderFactory } from './engineCameraStream.js';

/** A WebCodecs H.264 decoder for Annex-B pictures, hardware first. */
class WebCodecsDecoder implements CameraDecoder {
	private readonly waiting = new Set<() => void>();

	constructor(private readonly decoder: VideoDecoder) {
		decoder.addEventListener('dequeue', () => {
			this.settle();
		});
	}

	public decode(chunk: CameraChunk): void {
		if (this.decoder.state === 'configured') {
			this.decoder.decode(new EncodedVideoChunk({ type: chunk.key ? 'key' : 'delta', timestamp: chunk.timestamp, data: chunk.data }));
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
		return new WebCodecsDecoder(decoder);
	}
	return undefined;
};

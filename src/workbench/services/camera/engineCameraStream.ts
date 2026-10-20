import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { DataPacket } from '../../../platform/engine/common/dataChannel.js';
import { record } from '../../../platform/engine/common/engine.js';
import type { CameraEnd, ICameraStream } from './cameraService.js';
import { avcCodec } from './h264.js';

/** One encoded picture, as the decoder takes it. */
export interface CameraChunk {
	readonly key: boolean;
	readonly timestamp: number;
	/** Taken by the decoder: it may be detached once decode returns. */
	readonly data: Uint8Array;
}

/** The part of a WebCodecs `VideoDecoder` the stream uses. */
export interface CameraDecoder {
	decode(chunk: CameraChunk): void;
	/** Resolves once at most `limit` pictures wait to be decoded, or the decoder closed. */
	drained(limit: number): Promise<void>;
	close(): void;
}

/** A decoder for `codec`, or `undefined` when this computer can't decode it. */
export type CameraDecoderFactory = (codec: string, output: (frame: VideoFrame) => void, error: () => void) => Promise<CameraDecoder | undefined>;

/** Pictures allowed to wait for the decoder before the stream stops acknowledging. */
const decodeQueueLimit = 2;

const endReasons: readonly CameraEnd[] = ['stopped', 'withdrawn', 'lost'];

function metadata(packet: DataPacket): Record<string, unknown> {
	const value: unknown = JSON.parse(packet.metadata);
	if (!record(value)) {
		throw new Error('Invalid camera packet');
	}
	return value;
}

/**
 * A paired computer's camera over the engine's camera data channel. Flow
 * control is the channel's: a packet is acknowledged once it is queued for
 * decoding and the decoder keeps up, so a slow decoder backs up the host,
 * which then restarts from a keyframe instead of queuing stale pictures.
 */
export class EngineCameraStream extends Disposable implements ICameraStream {
	private readonly frameEmitter = this._register(new Emitter<VideoFrame>());
	public readonly onFrame = this.frameEmitter.event;
	private readonly endEmitter = this._register(new Emitter<CameraEnd>());
	public readonly onEnd = this.endEmitter.event;
	private channel: { close(): void } | undefined;
	private decoder: Promise<CameraDecoder | undefined> | undefined;
	private ended = false;

	constructor(private readonly createDecoder: CameraDecoderFactory) {
		super();
	}

	/** Takes the open channel; a stream that already ended closes it. */
	public attach(channel: { close(): void }): void {
		if (this.ended) {
			channel.close();
		} else {
			this.channel = channel;
		}
	}

	public async consume(packet: DataPacket): Promise<void> {
		if (this.ended) {
			return;
		}
		const fields = metadata(packet);
		if (packet.kind === 'exited') {
			const reason = endReasons.find(candidate => candidate === fields.reason);
			if (!reason) {
				throw new Error('Invalid camera end');
			}
			this.end(reason);
			return;
		}
		if (packet.kind !== 'camera' || !packet.data || typeof fields.keyframe !== 'boolean' || typeof fields.timestampUs !== 'string' || !/^\d{1,20}$/.test(fields.timestampUs)) {
			throw new Error('Invalid camera packet');
		}
		const key = fields.keyframe;
		if (!this.decoder) {
			// The decoder is configured from the first keyframe's parameter sets.
			const codec = key ? avcCodec(packet.data) : undefined;
			if (!codec) {
				return;
			}
			this.decoder = this.createDecoder(codec, frame => {
				this.deliver(frame);
			}, () => {
				this.end('cantShow');
			});
		}
		const decoder = await this.decoder;
		if (this.isEnded()) {
			return;
		}
		if (!decoder) {
			this.end('cantShow');
			return;
		}
		decoder.decode({ key, timestamp: Number(fields.timestampUs), data: packet.data });
		await decoder.drained(decodeQueueLimit);
	}

	private isEnded(): boolean {
		return this.ended;
	}

	/** The channel failed or stopped answering. */
	public lost(): void {
		this.end('lost');
	}

	private deliver(frame: VideoFrame): void {
		if (this.ended || !this.frameEmitter.hasListeners()) {
			frame.close();
			return;
		}
		this.frameEmitter.fire(frame);
	}

	private end(reason: CameraEnd): void {
		if (this.ended) {
			return;
		}
		this.release();
		this.endEmitter.fire(reason);
	}

	private release(): void {
		this.ended = true;
		this.channel?.close();
		this.channel = undefined;
		void this.decoder?.then(decoder => {
			decoder?.close();
		});
	}

	public override dispose(): void {
		this.release();
		super.dispose();
	}
}

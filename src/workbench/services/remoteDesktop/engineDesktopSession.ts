import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { DataPacket, DesktopMediaStatus } from '../../../platform/engine/common/dataChannel.js';
import { record, type DisplayInfo, type EngineCommand } from '../../../platform/engine/common/engine.js';
import type { CameraDecoder, CameraDecoderFactory } from '../camera/engineCameraStream.js';
import { avcCodec } from '../camera/h264.js';
import type { DesktopEnd, DesktopInput, DesktopPicture, DesktopStats, IDesktopSession } from './remoteDesktopService.js';

/** Pictures allowed to wait for the decoder before the channel stops acknowledging. */
const decodeQueueLimit = 2;

/**
 * The WebCodecs codec string for a desktop picture: H.264 from the
 * keyframe's own parameter sets, the others by their usual profile, which
 * WebCodecs checks before decoding. `undefined` until an H.264 keyframe.
 */
export function desktopCodec(codec: string, keyframe: Uint8Array): string | undefined {
	switch (codec) {
		case 'h264': return avcCodec(keyframe);
		case 'h265': return 'hev1.1.6.L153.B0';
		case 'vp8': return 'vp8';
		case 'vp9': return 'vp09.00.40.08';
		case 'av1': return 'av01.0.08M.08';
		default: return undefined;
	}
}

/** The picture's channel, as the session uses it. */
export interface DesktopVideoChannel {
	requestKeyframe(): void;
	mediaStatus(decodeUs: number): Promise<DesktopMediaStatus | undefined>;
	close(): void;
}

/** The two channels of a session: the picture, and for control the input. */
export interface DesktopChannels {
	video: DesktopVideoChannel;
	input: { input(command: EngineCommand): void; close(): void } | undefined;
}

/**
 * A paired computer's screen over the engine's `video` data channel, and its
 * keyboard and pointer over the `input` channel. Flow control is the
 * channel's: a picture is acknowledged once queued for decoding and the
 * decoder keeps up, so a slow decoder backs up the other computer, which
 * then restarts from a keyframe rather than queuing stale pictures.
 */
export class EngineDesktopSession extends Disposable implements IDesktopSession {
	private readonly pictureEmitter = this._register(new Emitter<DesktopPicture>());
	public readonly onPicture = this.pictureEmitter.event;
	private readonly endEmitter = this._register(new Emitter<DesktopEnd>());
	public readonly onEnd = this.endEmitter.event;
	private channels: DesktopChannels | undefined;
	private decoder: Promise<CameraDecoder | undefined> | undefined;
	private codec: string | undefined;
	/** Where each picture waiting in the decoder was taken, by its timestamp. */
	private readonly taken = new Map<number, { readonly display: string; readonly revision: string; readonly at: number }>();
	/** Time from decode to picture since stats were last read, to tell the engine how this computer keeps up. */
	private decodeTime = { total: 0, count: 0 };
	private ended = false;
	/** Whether a picture was decoded: after one, a decode error is a lost picture, not a codec this computer can't show. */
	private decodedAny = false;

	constructor(
		public readonly control: boolean,
		private readonly session: { readonly id: string; readonly generation: string },
		private readonly createDecoder: CameraDecoderFactory,
		private readonly readDisplays: () => Promise<readonly DisplayInfo[]>,
	) {
		super();
	}

	/** Takes the open channels; a session that already ended closes them. */
	public attach(channels: DesktopChannels): void {
		if (this.ended) {
			channels.video.close();
			channels.input?.close();
		} else {
			this.channels = channels;
		}
	}

	public displays(): Promise<readonly DisplayInfo[]> {
		return this.readDisplays();
	}

	public async stats(): Promise<DesktopStats | undefined> {
		const video = this.channels?.video;
		const codec = this.codec;
		if (!video || !codec || this.ended) {
			return undefined;
		}
		const { total, count } = this.decodeTime;
		this.decodeTime = { total: 0, count: 0 };
		const status = await video.mediaStatus(count ? total / count * 1000 : 0);
		return status && { status, codec };
	}

	public send(input: DesktopInput): void {
		const channel = this.channels?.input;
		if (!this.control || this.ended || !channel) {
			return;
		}
		try {
			channel.input({ ...input, sessionId: this.session.id, generation: this.session.generation });
		} catch {
			this.lost();
		}
	}

	public async consume(packet: DataPacket): Promise<void> {
		if (this.ended) {
			return;
		}
		const fields: unknown = JSON.parse(packet.metadata);
		if (packet.kind !== 'video' || !packet.data || !record(fields) || typeof fields.codec !== 'string' || typeof fields.keyframe !== 'boolean'
			|| typeof fields.timestampUs !== 'string' || !/^\d{1,20}$/.test(fields.timestampUs)
			|| typeof fields.displayId !== 'string' || typeof fields.revision !== 'string') {
			throw new Error('Invalid desktop packet');
		}
		const key = fields.keyframe;
		// A new codec, as after a reconnect negotiates another, starts a new decoder at its keyframe.
		if (!this.decoder || this.codec !== fields.codec) {
			const codec = key ? desktopCodec(fields.codec, packet.data) : undefined;
			if (!codec) {
				return;
			}
			void this.decoder?.then(previous => previous?.close());
			this.codec = fields.codec;
			this.taken.clear();
			const decoder = this.createDecoder(codec, frame => {
				this.decodedAny = true;
				this.deliver(frame);
			}, () => {
				this.decodeFailed(decoder);
			});
			this.decoder = decoder;
		}
		const decoder = await this.decoder;
		if (this.isEnded()) {
			return;
		}
		if (!decoder) {
			this.end('cantShow');
			return;
		}
		const timestamp = Number(fields.timestampUs);
		this.taken.set(timestamp, { display: fields.displayId, revision: fields.revision, at: performance.now() });
		decoder.decode({ key, timestamp, data: packet.data });
		await decoder.drained(decodeQueueLimit);
	}

	/**
	 * The decoder stopped: after a picture it lost its reference, so it
	 * starts again from a keyframe it asks for; before any, this computer
	 * can't show the picture.
	 */
	private decodeFailed(failed: Promise<CameraDecoder | undefined>): void {
		if (this.decoder !== failed || this.ended) {
			return;
		}
		if (!this.decodedAny) {
			this.end('cantShow');
			return;
		}
		void failed.then(decoder => decoder?.close());
		this.decoder = undefined;
		this.codec = undefined;
		this.taken.clear();
		this.channels?.video.requestKeyframe();
	}

	/** Read after an await: the session may have ended meanwhile. */
	private isEnded(): boolean {
		return this.ended;
	}

	/** The channel failed or stopped answering. */
	public lost(): void {
		this.end('lost');
	}

	private deliver(frame: VideoFrame): void {
		const taken = this.taken.get(frame.timestamp);
		// Pictures the decoder dropped before this one are not coming.
		for (const timestamp of this.taken.keys()) {
			if (timestamp <= frame.timestamp) {
				this.taken.delete(timestamp);
			}
		}
		if (taken) {
			this.decodeTime = { total: this.decodeTime.total + performance.now() - taken.at, count: this.decodeTime.count + 1 };
		}
		if (this.ended || !taken || !this.pictureEmitter.hasListeners()) {
			frame.close();
			return;
		}
		this.pictureEmitter.fire({ frame, display: taken.display, revision: taken.revision });
	}

	private end(reason: DesktopEnd): void {
		if (this.ended) {
			return;
		}
		this.release();
		this.endEmitter.fire(reason);
	}

	private release(): void {
		this.ended = true;
		this.channels?.video.close();
		this.channels?.input?.close();
		this.channels = undefined;
		this.taken.clear();
		void this.decoder?.then(decoder => {
			decoder?.close();
		});
	}

	public override dispose(): void {
		this.release();
		super.dispose();
	}
}

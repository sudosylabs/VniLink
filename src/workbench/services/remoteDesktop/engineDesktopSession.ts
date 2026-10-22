import { Emitter, type Event } from '../../../base/common/event.js';
import { Disposable, MutableDisposable } from '../../../base/common/lifecycle.js';
import type { DataPacket, DesktopCursor, DesktopCursorImage, DesktopDecoderReport, DesktopMediaStatus } from '../../../platform/engine/common/dataChannel.js';
import { record, videoCodecNames, type DisplayInfo, type EngineCommand, type VideoCodec } from '../../../platform/engine/common/engine.js';
import type { CameraDecoder, CameraDecoderFactory } from '../camera/engineCameraStream.js';
import { avcCodec } from '../camera/h264.js';
import { DesktopLatency } from './desktopLatency.js';
import { engineDesktopEnds, type DesktopEnd, type DesktopInput, type DesktopPicture, type DesktopPointerShape, type DesktopPointer, type DesktopStats, type IDesktopSession } from './remoteDesktopService.js';

/** Pictures allowed to wait for the decoder before the channel stops acknowledging. */
const decodeQueueLimit = 2;

function isVideoCodec(value: string | undefined): value is VideoCodec {
	return videoCodecNames.includes(value as VideoCodec);
}

/** A microsecond count the engine sends: a whole number from 0 to 2^32 − 1. */
function micros(value: unknown): value is number {
	return Number.isSafeInteger(value) && Number(value) >= 0 && Number(value) <= 0xffff_ffff;
}

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

/** The engine's premultiplied BGRA pointer image, as straight-alpha RGBA for a canvas. */
export function pointerShape(image: DesktopCursorImage): DesktopPointerShape {
	const rgba = new Uint8ClampedArray(image.width * image.height * 4);
	const pixels = image.pixels;
	for (let at = 0; at < rgba.length; at += 4) {
		const alpha = pixels[at + 3];
		const unpremultiply = (value: number): number => alpha ? Math.min(255, Math.round(value * 255 / alpha)) : 0;
		rgba[at] = unpremultiply(pixels[at + 2]);
		rgba[at + 1] = unpremultiply(pixels[at + 1]);
		rgba[at + 2] = unpremultiply(pixels[at]);
		rgba[at + 3] = alpha;
	}
	return { width: image.width, height: image.height, hotspotX: image.hotspotX, hotspotY: image.hotspotY, rgba };
}

/** The picture's channel, as the session uses it. */
export interface DesktopVideoChannel {
	/** The other computer's pointer, when it is left out of the picture. */
	readonly onCursor: Event<DesktopCursor>;
	requestKeyframe(): void;
	mediaStatus(decoder: DesktopDecoderReport): Promise<DesktopMediaStatus | undefined>;
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
	private readonly pointerEmitter = this._register(new Emitter<DesktopPointer>());
	public readonly onPointer = this.pointerEmitter.event;
	private readonly cursorListener = this._register(new MutableDisposable());
	private shape: DesktopPointerShape | undefined;
	private channels: DesktopChannels | undefined;
	private decoder: Promise<CameraDecoder | undefined> | undefined;
	private codec: string | undefined;
	/** Where each picture waiting in the decoder was taken, by its timestamp. */
	private readonly taken = new Map<number, { readonly display: string; readonly revision: string; readonly at: number }>();
	/**
	 * The decoder's time per picture while others wait behind it, since
	 * stats were last read: how fast this computer can decode, which the
	 * engine weighs against the frame rate. A picture's own wait in the
	 * decoder is latency, not load, and is left to the timing stages.
	 */
	private decodeService = { total: 0, count: 0 };
	/** When the decoder last put out a picture. */
	private lastOutputAt: number | undefined;
	private readonly latency = new DesktopLatency();
	private skippedPictures = 0;
	private ended = false;
	/** Whether a picture was decoded: after one, a decode error is a lost picture, not a codec this computer can't show. */
	private decodedAny = false;
	/** Times the decoder restarted from a keyframe after an error. */
	private recoveries = 0;
	/** Whether the current decoder fell back to software. */
	private software = false;

	constructor(
		public readonly control: boolean,
		private readonly session: { readonly id: string; readonly generation: string },
		private readonly createDecoder: CameraDecoderFactory,
		private readonly readDisplays: () => Promise<readonly DisplayInfo[]>,
	) {
		super();
	}

	/** The codec of the pictures being decoded, once one arrived. */
	public get shownCodec(): VideoCodec | undefined {
		return isVideoCodec(this.codec) ? this.codec : undefined;
	}

	/** Takes the open channels; a session that already ended closes them. */
	public attach(channels: DesktopChannels): void {
		if (this.ended) {
			channels.video.close();
			channels.input?.close();
		} else {
			this.channels = channels;
			this.cursorListener.value = channels.video.onCursor(cursor => {
				this.pointerMoved(cursor);
			});
		}
	}

	private pointerMoved(cursor: DesktopCursor): void {
		if (cursor.image) {
			this.shape = pointerShape(cursor.image);
		}
		this.pointerEmitter.fire({ x: cursor.x, y: cursor.y, visible: cursor.visible, shape: this.shape });
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
		const { total, count } = this.decodeService;
		this.decodeService = { total: 0, count: 0 };
		const status = await video.mediaStatus({ decodeUs: count ? total / count * 1000 : 0, software: this.software, recoveries: this.recoveries });
		const skipped = this.skippedPictures;
		this.skippedPictures = 0;
		return status && { status, codec, latency: this.latency.take(), skipped };
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
		// The channel's last packet says why the picture stopped.
		if (packet.kind === 'exited') {
			const end = record(fields) && Object.keys(fields).length === 1 ? engineDesktopEnds.find(known => known === fields.end) : undefined;
			if (!end) {
				throw new Error('Invalid desktop end');
			}
			this.end(end);
			return;
		}
		if (packet.kind !== 'video' || !packet.data || !record(fields) || typeof fields.codec !== 'string' || typeof fields.keyframe !== 'boolean'
			|| typeof fields.timestampUs !== 'string' || !/^\d{1,20}$/.test(fields.timestampUs)
			|| typeof fields.displayId !== 'string' || typeof fields.revision !== 'string'
			|| !micros(fields.hostUs) || !micros(fields.queuedUs)) {
			throw new Error('Invalid desktop packet');
		}
		// Zero is a picture sent without its capture time.
		if (fields.hostUs > 0) {
			this.latency.record('host', fields.hostUs / 1000);
		}
		this.latency.record('queue', fields.queuedUs / 1000);
		if (packet.readAt !== undefined) {
			this.latency.record('bridge', performance.timeOrigin + performance.now() - packet.readAt);
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
			this.lastOutputAt = undefined;
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
		this.software = decoder.software;
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
		this.recoveries++;
		void failed.then(decoder => decoder?.close());
		this.decoder = undefined;
		this.codec = undefined;
		this.taken.clear();
		this.lastOutputAt = undefined;
		this.channels?.video.requestKeyframe();
	}

	/** Read after an await: the session may have ended meanwhile. */
	private isEnded(): boolean {
		return this.ended;
	}

	/** The channel failed or stopped answering without saying why. */
	public lost(): void {
		this.end('linkLost');
	}

	private deliver(frame: VideoFrame): void {
		const taken = this.taken.get(frame.timestamp);
		let behind = 0;
		// Pictures the decoder dropped before this one are not coming.
		for (const timestamp of this.taken.keys()) {
			if (timestamp <= frame.timestamp) {
				this.taken.delete(timestamp);
			} else {
				behind++;
			}
		}
		const decodedAt = performance.now();
		// With pictures waiting, this one had the decoder from when it went
		// in or the previous one came out, whichever was later, until now.
		if (behind > 0 && taken && this.lastOutputAt !== undefined) {
			const started = Math.max(this.lastOutputAt, taken.at);
			this.decodeService = { total: this.decodeService.total + decodedAt - started, count: this.decodeService.count + 1 };
		}
		this.lastOutputAt = decodedAt;
		if (taken) {
			this.latency.record('decode', decodedAt - taken.at);
		}
		if (this.ended || !taken || !this.pictureEmitter.hasListeners()) {
			frame.close();
			return;
		}
		this.pictureEmitter.fire({ frame, display: taken.display, revision: taken.revision, decodedAt });
	}

	public presented(picture: DesktopPicture): void {
		this.latency.record('draw', performance.now() - picture.decodedAt);
	}

	public skipped(): void {
		this.skippedPictures++;
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
		this.cursorListener.clear();
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

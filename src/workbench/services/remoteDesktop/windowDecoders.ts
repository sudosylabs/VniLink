import { videoCodecNames, type VideoCodec, type WindowDecoder } from '../../../platform/engine/common/engine.js';

/**
 * The WebCodecs string each codec is asked about, as pictures are decoded
 * with them; H.264 by its High profile, which a decoder of Baseline also
 * decodes in practice.
 */
const probes: Readonly<Record<VideoCodec, string>> = {
	h264: 'avc1.640028',
	h265: 'hev1.1.6.L153.B0',
	vp8: 'vp8',
	vp9: 'vp09.00.40.08',
	av1: 'av01.0.08M.08',
};

/**
 * A still picture: AV1 High profile, 8-bit, 4:4:4 (chroma subsampling
 * `000`), asked about as the session's decoder opens it. See the host's
 * still pictures in the core's `FrameRole`.
 */
export const stillDecoderConfig: VideoDecoderConfig = { codec: 'av01.1.08M.08.0.000', hardwareAcceleration: 'no-preference', optimizeForLatency: true };

/** Whether this window can decode with a configuration. */
export type DecoderSupport = (config: VideoDecoderConfig) => Promise<boolean>;

/**
 * The codecs this window decodes, each marked when its decoder is hardware,
 * and AV1 when it also shows still pictures at 4:4:4.
 */
export async function probeWindowDecoders(supported: DecoderSupport): Promise<WindowDecoder[]> {
	const answers = await Promise.all(videoCodecNames.map(async codec => {
		const config = { codec: probes[codec], optimizeForLatency: true };
		const hardware = await supported({ ...config, hardwareAcceleration: 'prefer-hardware' });
		if (!hardware && !await supported({ ...config, hardwareAcceleration: 'prefer-software' })) {
			return undefined;
		}
		return { codec, hardware, chroma444: codec === 'av1' && await supported(stillDecoderConfig) };
	}));
	return answers.filter(answer => answer !== undefined);
}

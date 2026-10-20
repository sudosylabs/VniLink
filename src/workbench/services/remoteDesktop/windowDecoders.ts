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

/** Whether this window can decode with a configuration. */
export type DecoderSupport = (config: VideoDecoderConfig) => Promise<boolean>;

/** WebCodecs' own answer; a refusal to answer is no. */
export const webCodecsSupport: DecoderSupport = async config => {
	try {
		return (await VideoDecoder.isConfigSupported(config)).supported === true;
	} catch {
		return false;
	}
};

/** The codecs this window decodes, each marked when its decoder is hardware. */
export async function probeWindowDecoders(supported: DecoderSupport): Promise<WindowDecoder[]> {
	const answers = await Promise.all(videoCodecNames.map(async codec => {
		const config = { codec: probes[codec], optimizeForLatency: true };
		if (await supported({ ...config, hardwareAcceleration: 'prefer-hardware' })) {
			return { codec, hardware: true };
		}
		return await supported({ ...config, hardwareAcceleration: 'no-preference' }) ? { codec, hardware: false } : undefined;
	}));
	return answers.filter(answer => answer !== undefined);
}

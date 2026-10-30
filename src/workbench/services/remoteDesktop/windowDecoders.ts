// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { videoCodecNames, type VideoCodec, type WindowDecoder } from '../../../platform/engine/common/engine.js';
import type { StreamMatrix } from '../camera/decoderColour.js';

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

/**
 * The matrix this window decodes a configuration in, or `undefined` where it
 * cannot decode with it.
 */
export type DecoderSupport = (config: VideoDecoderConfig) => Promise<StreamMatrix | undefined>;

/**
 * The codecs this window decodes, each marked when its decoder is hardware
 * and when that decoder shows only BT.601 pictures correctly, and AV1 when it
 * also shows still pictures at 4:4:4.
 */
export async function probeWindowDecoders(supported: DecoderSupport): Promise<WindowDecoder[]> {
	const answers = await Promise.all(videoCodecNames.map(async codec => {
		const config = { codec: probes[codec], optimizeForLatency: true };
		const hardware = await supported({ ...config, hardwareAcceleration: 'prefer-hardware' });
		if (hardware === undefined && await supported({ ...config, hardwareAcceleration: 'prefer-software' }) === undefined) {
			return undefined;
		}
		return { codec, hardware: hardware !== undefined, chroma444: codec === 'av1' && await supported(stillDecoderConfig) !== undefined, bt601: hardware === 'bt601' };
	}));
	return answers.filter(answer => answer !== undefined);
}

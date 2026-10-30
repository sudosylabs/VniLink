// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { colourTolerance, hardwareMatrix, pictureColour, showsMatrix, shownColour, type StreamMatrix } from '../../src/workbench/services/camera/decoderColour.js';

function assert(condition: boolean, message: string): asserts condition {
	if (!condition) {
		throw new Error(message);
	}
}

/** One codec string per family with an embedded picture, as the window asks about them. */
const codecs = ['avc1.640028', 'hev1.1.6.L153.B0', 'vp8', 'vp09.00.40.08', 'av01.0.08M.08'];

/**
 * Each family's pictures decode in software, where this window has a
 * software decoder, to the colour they were made in, in either matrix, so
 * the check that finds the matrix a hardware decoder shows correctly
 * measures the right thing. VP8 cannot declare BT.601 and Chromium's software VP9
 * shows a BT.601 picture as BT.709 (measured on Chromium 152, Linux), so
 * neither is asked about it in software. What
 * hardware shows differs between computers and is not asserted, except that
 * the matrix it is given is one the check measures.
 */
export async function verifyDecoderColour(): Promise<void> {
	let checked = 0;
	for (const codec of codecs) {
		const software: VideoDecoderConfig = { codec, hardwareAcceleration: 'prefer-software', optimizeForLatency: true };
		if ((await VideoDecoder.isConfigSupported(software)).supported === true) {
			for (const matrix of ['bt709', 'bt601'] as const satisfies readonly StreamMatrix[]) {
				if ((codec === 'vp8' || codec.startsWith('vp09')) && matrix === 'bt601') {
					continue;
				}
				const shown = await shownColour(software, matrix);
				assert(shown !== undefined, `${codec}'s ${matrix} picture did not decode in software`);
				assert(shown.every((level, index) => Math.abs(level - pictureColour[index]) <= colourTolerance), `${codec} in software showed ${shown.join(', ')} for ${pictureColour.join(', ')} in ${matrix}`);
				assert(await showsMatrix(software, matrix), `${codec} in software was turned down in ${matrix}`);
			}
			assert(await hardwareMatrix(software) === 'bt709', `${codec} in software was not taken for BT.709`);
			checked++;
		}
	}
	assert(checked > 0, 'no codec decoded in software');
}

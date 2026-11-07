// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { colourTolerance, hardwareMatrix, pictureColour, shownColour, type StreamMatrix } from '../../decoderColour.js';

/**
 * One codec string per family with an embedded picture, as the window asks
 * about them. HEVC is left out: Chromium decodes it only in hardware, so it
 * has no software decoder to test.
 */
const codecs = ['avc1.640028', 'vp8', 'vp09.00.40.08', 'av01.0.08M.08'];

function software(codec: string): VideoDecoderConfig {
	return { codec, hardwareAcceleration: 'prefer-software', optimizeForLatency: true };
}

async function decodesInSoftware(codec: string): Promise<boolean> {
	return (await VideoDecoder.isConfigSupported(software(codec))).supported === true;
}

/**
 * Each family's pictures decode in software, where this window has a
 * software decoder, to the colour they were made in, in either matrix, so
 * the check that finds the matrix a hardware decoder shows correctly
 * measures the right thing. VP8 cannot declare BT.601 and Chromium's
 * software VP9 shows a BT.601 picture as BT.709, so neither is asked about
 * it in software. What hardware shows differs between computers and is not
 * asserted.
 */
suite('decoderColour', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('at least one codec decodes in software', async () => {
		const supported = await Promise.all(codecs.map(decodesInSoftware));
		assert.ok(supported.some(Boolean), 'no codec decoded in software');
	});
	for (const codec of codecs) {
		suite(codec, () => {
			setup(async function () {
				if (!await decodesInSoftware(codec)) {
					this.skip();
				}
			});
			for (const matrix of ['bt709', 'bt601'] as const satisfies readonly StreamMatrix[]) {
				if ((codec === 'vp8' || codec.startsWith('vp09')) && matrix === 'bt601') {
					continue;
				}
				test(`a ${matrix} picture decodes in software to the colour it was made in`, async () => {
					const shown = await shownColour(software(codec), matrix);
					assert.ok(shown, `the ${matrix} picture did not decode`);
					assert.ok(shown.every((level, index) => Math.abs(level - pictureColour[index]) <= colourTolerance), `showed ${shown.join(', ')} for ${pictureColour.join(', ')}`);
				});
			}
			test('a decoder that shows BT.709 correctly is taken for a BT.709 one', async () => {
				assert.strictEqual(await hardwareMatrix(software(codec)), 'bt709');
			});
		});
	}
});

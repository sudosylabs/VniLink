// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import type { StreamMatrix } from '../../../camera/decoderColour.js';
import { probeWindowDecoders } from '../../windowDecoders.js';

suite('probeWindowDecoders', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('the window says which codecs it decodes, hardware ones marked with their matrix, each once, and whether it shows stills', async () => {
		// A hardware decoder that shows neither matrix right answers nothing for `prefer-hardware`, as VP9's does here.
		// H.264 shows BT.709 in hardware; H.265 only BT.601, as Chromium's VA-API path converts.
		const answers: Partial<Record<string, Partial<Record<HardwareAcceleration, StreamMatrix>>>> = {
			'avc1.640028': { 'prefer-hardware': 'bt709', 'prefer-software': 'bt709' },
			'hev1.1.6.L153.B0': { 'prefer-hardware': 'bt601', 'prefer-software': 'bt709' },
			'vp09.00.40.08': { 'prefer-software': 'bt709' },
			'av01.0.08M.08': { 'prefer-software': 'bt709' },
			'av01.1.08M.08.0.000': { 'no-preference': 'bt709' },
		};
		const decoders = await probeWindowDecoders(config => Promise.resolve(answers[config.codec]?.[config.hardwareAcceleration ?? 'no-preference']));
		assert.deepStrictEqual(decoders, [
			{ codec: 'h264', hardware: true, chroma444: false, bt601: false },
			{ codec: 'h265', hardware: true, chroma444: false, bt601: true },
			{ codec: 'vp9', hardware: false, chroma444: false, bt601: false },
			{ codec: 'av1', hardware: false, chroma444: true, bt601: false },
		]);
	});
});

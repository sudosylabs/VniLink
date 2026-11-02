// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { avcCodec } from '../../h264.js';

suite('avcCodec', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const cases: readonly (readonly [string, Uint8Array, string | undefined])[] = [
		['a Constrained Baseline keyframe after a four-byte start code', Uint8Array.from([0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0xda, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 1, 0x65, 0x88]), 'avc1.42C01F'],
		['a High profile parameter set after a three-byte start code', Uint8Array.from([0, 0, 1, 0x27, 0x64, 0x00, 0x28]), 'avc1.640028'],
		['a predicted picture', Uint8Array.from([0, 0, 0, 1, 0x41, 0x9a]), undefined],
		['a truncated parameter set', Uint8Array.from([0, 0, 1, 0x67, 0x42]), undefined],
	];
	for (const [name, data, codec] of cases) {
		test(`${name} names ${codec ?? 'no codec'}`, () => {
			assert.strictEqual(avcCodec(data), codec);
		});
	}
});

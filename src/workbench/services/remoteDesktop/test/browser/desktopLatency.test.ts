// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { DesktopLatency } from '../../desktopLatency.js';

suite('DesktopLatency', () => {
	test('stage times summarise as the median and the time nineteen in twenty beat', () => {
		const latency = new DesktopLatency();
		for (let ms = 1; ms <= 100; ms++) {
			latency.record('decode', ms);
		}
		assert.deepStrictEqual(latency.take(), { decode: { p50: 50, p95: 95, count: 100 } });
	});
	for (const ms of [-3, Number.NaN]) {
		test(`a time of ${String(ms)}, from clocks out of step, is left out`, () => {
			const latency = new DesktopLatency();
			latency.record('host', ms);
			assert.deepStrictEqual(latency.take(), {});
		});
	}
	test('taking the summary starts afresh', () => {
		const latency = new DesktopLatency();
		latency.record('decode', 1);
		latency.take();
		assert.deepStrictEqual(latency.take(), {});
	});
	test('only the newest pictures count', () => {
		const latency = new DesktopLatency();
		for (let at = 0; at < 300; at++) {
			latency.record('draw', at < 60 ? 1000 : 1);
		}
		assert.deepStrictEqual(latency.take(), { draw: { p50: 1, p95: 1, count: 240 } });
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

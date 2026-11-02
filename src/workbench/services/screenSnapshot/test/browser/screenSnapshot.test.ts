// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { parseScreenSnapshot } from '../../screenSnapshot.js';

/** The smallest JPEG that states its size: start, a baseline frame header of `width`×`height`, end. */
function jpeg(width: number, height: number): Uint8Array {
	return new Uint8Array([
		0xff, 0xd8, 0xff, 0xc0, 0x00, 0x0b, 0x08,
		(height >> 8) & 0xff, height & 0xff,
		(width >> 8) & 0xff, width & 0xff,
		0x01, 0x01, 0x11, 0x00, 0xff, 0xd9,
	]);
}

suite('parseScreenSnapshot', () => {
	const image = jpeg(8, 8);
	const ready = { outcome: 'ready', width: 8, height: 8, capturedUnixMs: '1700000000000', displayId: 'display-7', mime: 'image/jpeg', endpointId: 'a'.repeat(64), sessionId: '1', generation: '2' };
	test('a ready snapshot carries its size, capture time, computer and generation', () => {
		const parsed = parseScreenSnapshot(JSON.stringify(ready), image);
		assert.ok(parsed?.status === 'ready');
		assert.strictEqual(parsed.width, 8);
		assert.strictEqual(parsed.capturedUnixMs, 1700000000000);
		assert.strictEqual(parsed.endpointId, 'a'.repeat(64));
		assert.strictEqual(parsed.generation, '2');
	});
	const refusedMetadata: [string, object][] = [
		['an extra field', { ...ready, extra: true }],
		['a width beyond the bound', { ...ready, width: 641 }],
		['another image type', { ...ready, mime: 'image/png' }],
		['a numeric capture time', { ...ready, capturedUnixMs: 1700000000000 }],
	];
	for (const [name, metadata] of refusedMetadata) {
		test(`a ready snapshot with ${name} is refused`, () => {
			assert.strictEqual(parseScreenSnapshot(JSON.stringify(metadata), image), undefined);
		});
	}
	const refusedImages: [string, Uint8Array][] = [
		['a JPEG without a frame header', new Uint8Array([0xff, 0xd8, 0x00, 0xff, 0xd9])],
		['a JPEG of another size than it says', jpeg(4, 4)],
		['a JPEG beyond the bound', jpeg(800, 8)],
	];
	for (const [name, data] of refusedImages) {
		test(`a ready snapshot carrying ${name} is refused`, () => {
			assert.strictEqual(parseScreenSnapshot(JSON.stringify(ready), data), undefined);
		});
	}
	for (const outcome of ['needsPermission', 'needsSourceSelection', 'cancelled', 'replaced'] as const) {
		test(`the outcome ${outcome} is a status without a picture`, () => {
			assert.deepStrictEqual(parseScreenSnapshot(JSON.stringify({ outcome }), undefined), { status: outcome });
		});
	}
	const refusedOutcomes: [string, object][] = [
		['an outcome with a picture field', { outcome: 'needsPermission', width: 1 }],
		['an unknown outcome', { outcome: 'asleep' }],
		['an outcome the engine never sends', { outcome: 'allowed' }],
	];
	for (const [name, metadata] of refusedOutcomes) {
		test(`${name} is refused`, () => {
			assert.strictEqual(parseScreenSnapshot(JSON.stringify(metadata), undefined), undefined);
		});
	}
	ensureNoDisposablesAreLeakedInTestSuite();
});

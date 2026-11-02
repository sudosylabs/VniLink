// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { readFile } from 'node:fs/promises';
import { suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { trayFileName, trayStates, trayVariants } from '../trayAssets.ts';

const assets = fileURLToPath(new URL('../../../assets/tray/', import.meta.url));

/** A PNG's width and height, read from its header. */
function pngSize(bytes: Buffer): [number, number] {
	assert.strictEqual(bytes.subarray(1, 4).toString('latin1'), 'PNG');
	return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
}

suite('trayAssets', () => {
	for (const variant of trayVariants) {
		for (const state of trayStates) {
			for (const scale of variant.scales) {
				const name = trayFileName(variant, state, scale);
				test(`${name} is rendered at its scale`, async () => {
					const pixels = Math.round(variant.size * scale);
					assert.deepStrictEqual(pngSize(await readFile(`${assets}${name}`)), [pixels, pixels]);
				});
			}
		}
	}
	test('renders at 1x exactly the artwork the tray service asks for', () => {
		// The names `trayImageName` in `trayMainService.ts` produces; its own test lists the same.
		const requested = ['idleTemplate.png', 'activeTemplate.png', 'pausedTemplate.png', 'idle-dark.png', 'active-dark.png', 'paused-dark.png', 'idle-light.png', 'active-light.png', 'paused-light.png'];
		const rendered = trayVariants.flatMap(variant => trayStates.map(state => trayFileName(variant, state, 1)));
		assert.deepStrictEqual(rendered.sort(), requested.sort());
	});
});

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
import { deflateSync } from 'node:zlib';
import { decodePng, encodeBmp, installerImageFile, installerImages } from '../installerAssets.ts';
import * as darwin from '../package.darwin.ts';
import * as win32 from '../package.win32.ts';

const assets = fileURLToPath(new URL('../../../assets/', import.meta.url));

/** A PNG's width and height, read from its header. */
function pngSize(bytes: Buffer): [number, number] {
	assert.strictEqual(bytes.subarray(1, 4).toString('latin1'), 'PNG');
	return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
}

/** A one-chunk RGBA PNG of `rows`, each row given its filter byte. */
function png(width: number, height: number, rows: readonly (readonly number[])[]): Buffer {
	const chunk = (type: string, body: Buffer) => {
		const length = Buffer.alloc(4);
		length.writeUInt32BE(body.length);
		// The decoder does not check CRCs.
		return Buffer.concat([length, Buffer.from(type, 'latin1'), body, Buffer.alloc(4)]);
	};
	const header = Buffer.alloc(13);
	header.writeUInt32BE(width, 0);
	header.writeUInt32BE(height, 4);
	header[8] = 8;
	header[9] = 6;
	return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', header), chunk('IDAT', deflateSync(Buffer.from(rows.flat()))), chunk('IEND', Buffer.alloc(0))]);
}

suite('installerAssets', () => {
	for (const image of installerImages) {
		for (const scale of image.scales) {
			const name = installerImageFile(image, scale);
			test(`${name} is rendered at its size`, async () => {
				const bytes = await readFile(`${assets}${name}`);
				if (image.format === 'png') {
					assert.deepStrictEqual(pngSize(bytes), [image.width * scale, image.height * scale]);
				} else {
					assert.strictEqual(bytes.toString('latin1', 0, 2), 'BM');
					assert.deepStrictEqual([bytes.readInt32LE(18), bytes.readInt32LE(22), bytes.readUInt16LE(28)], [image.width, image.height, 24]);
				}
			});
		}
	}
	test('the macOS installer stages every macOS image at each scale', () => {
		const rendered = installerImages.filter(image => image.path.startsWith('apple/')).flatMap(image => image.scales.map(scale => `assets/${installerImageFile(image, scale)}`));
		assert.deepStrictEqual([...darwin.resources].sort(), rendered.sort());
	});
	test('the Windows installer stages every Windows image', () => {
		const rendered = installerImages.filter(image => image.path.startsWith('windows/')).map(image => `assets/${image.path}`);
		assert.deepStrictEqual(win32.resources.filter(path => path.startsWith('assets/')).sort(), rendered.sort());
	});
	test('undoes each PNG row filter', () => {
		// One row per filter: none, sub, up, average and Paeth, each two pixels wide.
		const rows = [
			[0, 10, 20, 30, 255, 40, 50, 60, 255],
			[1, 10, 20, 30, 255, 1, 1, 1, 0],
			[2, 0, 0, 0, 0, 0, 0, 0, 0],
			[3, 2, 2, 2, 0, 2, 2, 2, 0],
			[4, 0, 0, 0, 0, 0, 0, 0, 0],
		];
		const { rgba } = decodePng(png(2, 5, rows));
		assert.deepStrictEqual([...rgba.subarray(0, 8)], [10, 20, 30, 255, 40, 50, 60, 255]);
		assert.deepStrictEqual([...rgba.subarray(8, 16)], [10, 20, 30, 255, 11, 21, 31, 255]);
		assert.deepStrictEqual([...rgba.subarray(16, 24)], [10, 20, 30, 255, 11, 21, 31, 255]);
		assert.deepStrictEqual([...rgba.subarray(24, 32)], [7, 12, 17, 127, 11, 18, 26, 191]);
		assert.deepStrictEqual([...rgba.subarray(32, 40)], [7, 12, 17, 127, 11, 18, 26, 191]);
	});
	test('writes BMP rows bottom-up in blue, green, red order over white, padded to four bytes', () => {
		const bmp = encodeBmp(1, 2, Buffer.from([255, 0, 0, 255, 0, 0, 0, 0]));
		assert.deepStrictEqual([...bmp.subarray(54, 58)], [255, 255, 255, 0]);
		assert.deepStrictEqual([...bmp.subarray(58, 62)], [0, 0, 255, 0]);
	});
});

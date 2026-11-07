// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { assignExecutableUuids, derivedUuid, executableUuids, formatUuid, machoUuids } from '../macos/identity.ts';
import { isMacintosh } from '../platform.ts';
import { loadProduct } from '../product.ts';

const product = await loadProduct(fileURLToPath(new URL('../../../', import.meta.url)));

/** A 64-bit little-endian Mach-O with a segment command before its LC_UUID. */
function thin(uuidByte: number): Buffer {
	const binary = Buffer.alloc(32 + 72 + 24 + 16);
	binary.writeUInt32LE(0xfeedfacf, 0);
	binary.writeUInt32LE(2, 16);
	binary.writeUInt32LE(72 + 24, 20);
	binary.writeUInt32LE(0x19, 32);
	binary.writeUInt32LE(72, 36);
	binary.writeUInt32LE(0x1b, 104);
	binary.writeUInt32LE(24, 108);
	binary.fill(uuidByte, 112, 128);
	binary.fill(0xee, 128);
	return binary;
}

/** A universal binary of two thin slices. */
function fat(first: Buffer, second: Buffer): Buffer {
	const header = Buffer.alloc(64);
	header.writeUInt32BE(0xcafebabe, 0);
	header.writeUInt32BE(2, 4);
	header.writeUInt32BE(64, 16);
	header.writeUInt32BE(first.length, 20);
	header.writeUInt32BE(64 + first.length, 36);
	header.writeUInt32BE(second.length, 40);
	return Buffer.concat([header, first, second]);
}

suite('macOS identity', () => {
	test('finds the UUID of a thin executable', () => {
		assert.deepStrictEqual(machoUuids(thin(0xab)).map(slice => [slice.offset, Buffer.from(slice.uuid).toString('hex')]), [[112, 'ab'.repeat(16)]]);
	});
	test('finds the UUID of each architecture in a universal executable', () => {
		assert.deepStrictEqual(machoUuids(fat(thin(0x11), thin(0x22))).map(slice => slice.offset), [64 + 112, 64 + thin(0).length + 112]);
	});
	test('refuses a file that is not a Mach-O executable', () => {
		assert.throws(() => machoUuids(Buffer.from('#!/bin/sh\n')), /Not a Mach-O/);
	});
	test('refuses an executable without an LC_UUID', () => {
		const noUuid = thin(0);
		noUuid.writeUInt32LE(1, 16);
		assert.throws(() => machoUuids(noUuid), /no LC_UUID/);
	});
	const stock = Buffer.alloc(16, 0x4c);
	test('derives the same UUID for a bundle identifier on every build', () => {
		assert.deepStrictEqual(derivedUuid(product.appId, stock), derivedUuid(product.appId, stock));
	});
	test('derives another UUID for another bundle identifier', () => {
		assert.notDeepStrictEqual(derivedUuid(`${product.appId}.dev`, stock), derivedUuid(product.appId, stock));
	});
	test('derives another UUID for another stock UUID', () => {
		assert.notDeepStrictEqual(derivedUuid(product.appId, Buffer.alloc(16, 0x4d)), derivedUuid(product.appId, stock));
	});
	test('derives an RFC 9562 version 8 UUID', () => {
		assert.match(formatUuid(derivedUuid(product.appId, stock)), /^[0-9A-F]{8}-[0-9A-F]{4}-8[0-9A-F]{3}-[89AB][0-9A-F]{3}-[0-9A-F]{12}$/);
	});
	suite('assigning UUIDs', () => {
		let directory: string;
		let path: string;
		const original = fat(thin(0x11), thin(0x22));
		beforeEach(async () => {
			directory = await mkdtemp(join(tmpdir(), 'vnilink-identity-'));
			path = join(directory, 'Electron');
			await writeFile(path, original);
		});
		afterEach(async () => {
			await rm(directory, { recursive: true, force: true });
		});
		test('assigns the UUIDs it predicts', async () => {
			const expected = await executableUuids(path, product.appId);
			assert.deepStrictEqual(await assignExecutableUuids(path, product.appId), expected);
		});
		test('assigns each architecture a UUID of its own', async () => {
			assert.strictEqual(new Set(await assignExecutableUuids(path, product.appId)).size, 2);
		});
		test('writes the assigned UUIDs into the executable', async () => {
			const assigned = await assignExecutableUuids(path, product.appId);
			assert.deepStrictEqual(machoUuids(await readFile(path)).map(slice => formatUuid(slice.uuid)), assigned);
		});
		test('changes nothing in the executable but the UUIDs', async () => {
			await assignExecutableUuids(path, product.appId);
			const changed = await readFile(path);
			const expected = Buffer.from(original);
			for (const slice of machoUuids(changed)) {
				changed.fill(0, slice.offset, slice.offset + 16);
				expected.fill(0, slice.offset, slice.offset + 16);
			}
			assert.deepStrictEqual(changed, expected);
		});
	});
	const electron = () => createRequire(import.meta.url)('electron') as string;
	test('finds a UUID in the stock Electron executable', { skip: !isMacintosh }, async () => {
		assert.ok((await executableUuids(electron(), product.appId)).length >= 1);
	});
	test('gives the stock Electron executable UUIDs other than its own', { skip: !isMacintosh }, async () => {
		const stockUuids = machoUuids(await readFile(electron())).map(slice => formatUuid(slice.uuid));
		assert.notDeepStrictEqual(await executableUuids(electron(), product.appId), stockUuids);
	});
});

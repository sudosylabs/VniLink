import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { assignExecutableUuids, darwinUsageDescriptions, derivedUuid, executableUuids, formatUuid, machoUuids } from '../../build/lib/macos/identity.ts';
import { packageConfiguration } from '../../build/lib/package.ts';

const product = JSON.parse(await readFile(new URL('../../product.json', import.meta.url), 'utf8'));

/** A 64-bit little-endian Mach-O with a segment command before its LC_UUID. */
function thin(uuidByte) {
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
function fat(first, second) {
	const header = Buffer.alloc(64);
	header.writeUInt32BE(0xcafebabe, 0);
	header.writeUInt32BE(2, 4);
	header.writeUInt32BE(64, 16);
	header.writeUInt32BE(first.length, 20);
	header.writeUInt32BE(64 + first.length, 36);
	header.writeUInt32BE(second.length, 40);
	return Buffer.concat([header, first, second]);
}

test('each architecture\'s UUID is found in thin and universal executables', () => {
	assert.deepEqual(machoUuids(thin(0xab)).map(slice => [slice.offset, Buffer.from(slice.uuid).toString('hex')]), [[112, 'ab'.repeat(16)]]);
	const universal = fat(thin(0x11), thin(0x22));
	assert.deepEqual(machoUuids(universal).map(slice => slice.offset), [64 + 112, 64 + thin(0).length + 112]);
	assert.throws(() => machoUuids(Buffer.from('#!/bin/sh\n')), /Not a Mach-O/);
	const noUuid = thin(0);
	noUuid.writeUInt32LE(1, 16);
	assert.throws(() => machoUuids(noUuid), /no LC_UUID/);
});

test('a bundle identifier gets its own UUID, the same on every build', () => {
	const stock = Buffer.alloc(16, 0x4c);
	const release = derivedUuid('fr.sudosy.vnilink', stock);
	assert.deepEqual(derivedUuid('fr.sudosy.vnilink', stock), release);
	assert.notDeepEqual(derivedUuid('fr.sudosy.vnilink.dev', stock), release);
	assert.notDeepEqual(derivedUuid('fr.sudosy.vnilink', Buffer.alloc(16, 0x4d)), release);
	// RFC 9562 version 8, variant 10.
	assert.match(formatUuid(release), /^[0-9A-F]{8}-[0-9A-F]{4}-8[0-9A-F]{3}-[89AB][0-9A-F]{3}-[0-9A-F]{12}$/);
});

test('assigning changes only each architecture\'s UUID', async () => {
	const directory = await mkdtemp(join(tmpdir(), 'vnilink-identity-'));
	try {
		const path = join(directory, 'Electron');
		const original = fat(thin(0x11), thin(0x22));
		await writeFile(path, original);
		const expected = await executableUuids(path, 'fr.sudosy.vnilink');
		assert.deepEqual(await assignExecutableUuids(path, 'fr.sudosy.vnilink'), expected);
		assert.equal(new Set(expected).size, 2);
		const changed = await readFile(path);
		assert.deepEqual(machoUuids(changed).map(slice => formatUuid(slice.uuid)), expected);
		for (const slice of machoUuids(changed)) {
			changed.fill(0, slice.offset, slice.offset + 16);
			original.fill(0, slice.offset, slice.offset + 16);
		}
		assert.deepEqual(changed, original);
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});

test('the stock Electron executable has a UUID to replace', { skip: process.platform !== 'darwin' }, async () => {
	const stock = createRequire(import.meta.url)('electron');
	const uuids = await executableUuids(stock, 'fr.sudosy.vnilink');
	assert.ok(uuids.length >= 1);
	assert.notDeepEqual(uuids, machoUuids(await readFile(stock)).map(slice => formatUuid(slice.uuid)));
});

test('the release declares every macOS usage description, local network included', () => {
	const config = packageConfiguration('/repo', '/stage', '/output', product, { platform: 'darwin', electronVersion: '44.7.0' });
	assert.deepEqual(config.mac.extendInfo, darwinUsageDescriptions);
	assert.match(darwinUsageDescriptions.NSLocalNetworkUsageDescription, /network/);
});

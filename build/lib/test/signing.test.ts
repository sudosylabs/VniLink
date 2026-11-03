// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { readFile } from 'node:fs/promises';
import { suite, test } from 'node:test';
import { entitlements, macSigning, notarizeDiskImage, signingKind, signingKinds } from '../signing.ts';

const configured = { VNILINK_MACOS_SIGNING_IDENTITY: 'Developer ID Application: Test', VNILINK_MACOS_NOTARY_PROFILE: 'test-profile' };

suite('signing', () => {
	test('a build without signing variables is unsigned', () => {
		assert.strictEqual(macSigning({}), undefined);
		assert.strictEqual(signingKind('darwin', {}), 'unsigned');
	});
	test('a macOS build with an identity and a notary profile is signed with them', () => {
		assert.deepStrictEqual(macSigning(configured), { identity: 'Developer ID Application: Test', notaryProfile: 'test-profile' });
		assert.strictEqual(signingKind('darwin', configured), 'developer-id');
	});
	test('an identity without a notary profile is refused rather than half signing', () => {
		assert.throws(() => macSigning({ VNILINK_MACOS_SIGNING_IDENTITY: 'Developer ID Application: Test' }), /set both or neither/);
	});
	test('the macOS variables sign nothing on another platform', () => {
		assert.strictEqual(signingKind('linux', configured), 'unsigned');
		assert.deepStrictEqual(signingKinds('linux'), ['unsigned']);
	});
	test('the hardened runtime allows JIT and the shared devices, nothing else', async () => {
		const keys = [...(await readFile(entitlements, 'utf8')).matchAll(/<key>([^<]+)<\/key>/g)].map(match => match[1]);
		assert.deepStrictEqual(keys, ['com.apple.security.cs.allow-jit', 'com.apple.security.device.camera', 'com.apple.security.device.audio-input']);
	});
	test('a disk image is signed, notarized and stapled in that order', async () => {
		const commands: string[] = [];
		await notarizeDiskImage({ run: (command, args) => Promise.resolve(commands.push([command, ...args].join(' '))) }, '/out/VniLink.dmg', { identity: 'Developer ID Application: Test', notaryProfile: 'test-profile' });
		assert.deepStrictEqual(commands, [
			'codesign --sign Developer ID Application: Test --timestamp /out/VniLink.dmg',
			'xcrun notarytool submit /out/VniLink.dmg --keychain-profile test-profile --wait',
			'xcrun stapler staple /out/VniLink.dmg',
		]);
	});
});

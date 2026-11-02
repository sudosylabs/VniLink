// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isEngineCommand, isEngineReply } from '../../common/engine.js';

suite('Engine desktop contracts', () => {
	const session = { id: '1', generation: '7', role: 'Controller', peer_id: 'peer', phase: 'Active' };
	test('a session names its connection', () => {
		assert.strictEqual(isEngineReply({ type: 'sessions', sessions: [session] }), true);
	});
	for (const extra of [{ quality: null }, { monitor_width: 1920 }, { monitor_height: 1080 }]) {
		test(`a session with ${Object.keys(extra)[0]} is refused, as it names nothing of a picture`, () => {
			assert.strictEqual(isEngineReply({ type: 'sessions', sessions: [{ ...session, ...extra }] }), false);
		});
	}
	for (const command of [
		{ type: 'updateSessionQuality', sessionId: '1', generation: '7', role: 'Controller', quality: { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 } },
		{ type: 'selectDisplays', sessionId: '1', generation: '7', ids: ['1'] },
	]) {
		test(`${command.type} is no engine command`, () => {
			assert.strictEqual(isEngineCommand(command), false);
		});
	}
	for (const codec of ['auto', 'h264', 'h265', 'vp8', 'vp9', 'av1']) {
		test(`'${codec}' is a codec preference`, () => {
			assert.strictEqual(isEngineCommand({ type: 'setDesktopCodecDefault', codec }), true);
		});
	}
	const malformedPreferences: readonly (readonly [string, unknown])[] = [
		['an unknown codec', { type: 'setDesktopCodecDefault', codec: 'bogus' }],
		['a missing codec', { type: 'setDesktopCodecDefault' }],
		['an extra field', { type: 'setDesktopCodecDefault', codec: 'vp9', sessionId: '1' }],
	];
	for (const [name, command] of malformedPreferences) {
		test(`a codec preference with ${name} is refused`, () => {
			assert.strictEqual(isEngineCommand(command), false);
		});
	}
	test('the decoders a window found are a valid registration', () => {
		const decoders = [
			{ codec: 'h264', hardware: true, chroma444: false, bt601: false },
			{ codec: 'h265', hardware: true, chroma444: false, bt601: true },
			{ codec: 'vp9', hardware: false, chroma444: false, bt601: false },
			{ codec: 'av1', hardware: false, chroma444: true, bt601: false },
		];
		assert.strictEqual(isEngineCommand({ type: 'setWindowDecoders', decoders }), true);
	});
	test('a window that decodes nothing may say so', () => {
		assert.strictEqual(isEngineCommand({ type: 'setWindowDecoders', decoders: [] }), true);
	});
	const malformedDecoders: readonly (readonly [string, unknown[]])[] = [
		['a codec listed twice', [{ codec: 'h264', hardware: true, chroma444: false, bt601: false }, { codec: 'h264', hardware: false, chroma444: false, bt601: false }]],
		['an unknown codec', [{ codec: 'mjpeg', hardware: true, chroma444: false, bt601: false }]],
		['no chroma444', [{ codec: 'h264', hardware: true, bt601: false }]],
		['no bt601', [{ codec: 'h264', hardware: true, chroma444: false }]],
		['a chroma444 that is not a boolean', [{ codec: 'h264', hardware: true, chroma444: 1, bt601: false }]],
		['a bt601 that is not a boolean', [{ codec: 'h264', hardware: true, chroma444: false, bt601: 1 }]],
		['an extra field', [{ codec: 'h264', hardware: true, chroma444: false, bt601: false, extra: 1 }]],
	];
	for (const [name, decoders] of malformedDecoders) {
		test(`a decoder registration with ${name} is refused`, () => {
			assert.strictEqual(isEngineCommand({ type: 'setWindowDecoders', decoders }), false);
		});
	}
	ensureNoDisposablesAreLeakedInTestSuite();
});

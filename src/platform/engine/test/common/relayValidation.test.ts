// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import type { RelayInfo, RelayInput } from '../../common/engine.js';
import { relaysChanged, validateRelays } from '../../common/relayValidation.js';

function relay(url: string, authenticated = false, replacementToken = ''): RelayInput {
	return { url, authenticated, replacementToken };
}

suite('validateRelays', () => {
	const saved = new Set(['https://saved.example/']);
	for (const url of ['https://relay.example/', 'http://localhost:3340/', 'http://127.0.0.2/', 'http://[::1]/']) {
		test(`${url} is accepted`, () => {
			assert.deepStrictEqual(validateRelays([relay(url)], saved), []);
		});
	}
	for (const url of ['http://remote.example/', 'https://name:secret@relay.example/', 'https://relay.example/?', 'https://relay.example/#', 'https://relay.example:0/', ' https://relay.example/', 'https://relay.example/with space']) {
		test(`'${url}' is refused as a URL`, () => {
			assert.strictEqual(validateRelays([relay(url)], saved)[0]?.field, 'url');
		});
	}
	test('an empty list is refused', () => {
		assert.strictEqual(validateRelays([], saved).length, 1);
	});
	test('more than eight relays are refused', () => {
		assert.strictEqual(validateRelays(Array.from({ length: 9 }, (_, i) => relay(`https://relay${String(i)}.example/`)), saved).length, 1);
	});
	test('the same relay twice is refused, however its URL is written', () => {
		assert.strictEqual(validateRelays([relay('https://relay.example'), relay('https://relay.example/')], saved).length, 1);
	});
	test('an authenticated relay keeps its saved token for the same URL', () => {
		assert.deepStrictEqual(validateRelays([relay('https://saved.example/', true)], saved), []);
	});
	for (const [name, input] of [
		['an authenticated relay at a new URL without a token', relay('https://changed.example/', true)],
		['a token with whitespace', relay('https://saved.example/', true, 'has space')],
		['a token longer than 256 bytes', relay('https://saved.example/', true, 'é'.repeat(129))],
	] as const) {
		test(`${name} is refused as a token`, () => {
			assert.strictEqual(validateRelays([input], saved)[0]?.field, 'token');
		});
	}
	ensureNoDisposablesAreLeakedInTestSuite();
});

suite('relaysChanged', () => {
	const saved: RelayInfo[] = [{ url: 'https://relay.example.com/', authenticated: true }];
	test('a draft that differs only in how its URL is written is unchanged', () => {
		assert.strictEqual(relaysChanged(saved, [relay('https://RELAY.example.com', true)]), false);
	});
	test('a new credential for a saved relay is an edit', () => {
		assert.strictEqual(relaysChanged(saved, [relay('https://relay.example.com/', true, 'new-token')]), true);
	});
	test('removing every relay is an edit', () => {
		assert.strictEqual(relaysChanged(saved, []), true);
	});
	test('two empty lists are unchanged', () => {
		assert.strictEqual(relaysChanged([], []), false);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

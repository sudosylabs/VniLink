// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { suite, test } from 'node:test';
import { formatTypeScript } from '../format.ts';

suite('format', () => {
	const literal = 'function example(){\n  const text = `line with spaces  \nnext line`;\n  return text;\n}\n';
	const compressed = 'class Owner { value = "a; b"; start() { const one = 1; const two = 2; return [one,two]; } stop() { return this.value; } }';
	const control = 'try { work(); }\ncatch (error) { fail(error); }\nfinally { stop(); }\nif (ready) { work(); }\nelse { stop(); }';
	test('indents with tabs', () => {
		assert.match(formatTypeScript('/tmp/format-fixture.ts', literal), /\n\tconst text/);
	});
	test('preserves whitespace inside template literals', () => {
		assert.ok(formatTypeScript('/tmp/format-fixture.ts', literal).includes('line with spaces  \nnext line'));
	});
	test('puts each statement of a compressed block on its own line', () => {
		assert.match(formatTypeScript('/tmp/owner.ts', compressed), /const one = 1;\n\s+const two = 2;/);
	});
	test('separates class members with a blank line', () => {
		assert.match(formatTypeScript('/tmp/owner.ts', compressed), /\n\n\s+stop/);
	});
	test('preserves string literal content', () => {
		assert.ok(formatTypeScript('/tmp/owner.ts', compressed).includes('"a; b"'));
	});
	for (const keyword of ['catch', 'finally', 'else']) {
		test(`keeps ${keyword} beside the preceding block`, () => {
			assert.match(formatTypeScript('/tmp/control.ts', control), new RegExp(`} ${keyword}`));
		});
	}
	for (const [name, input] of [['literal', literal], ['compressed', compressed], ['control flow', control]] as const) {
		test(`is idempotent on ${name} code`, () => {
			const formatted = formatTypeScript('/tmp/fixture.ts', input);
			assert.strictEqual(formatTypeScript('/tmp/fixture.ts', formatted), formatted);
		});
	}
});

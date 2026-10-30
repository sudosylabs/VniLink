// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import { join, relative } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

/** The methods that manage the engine's span itself. */
const SPAN_OWNERS = new Set(['trace_span', 'set_trace_name']);

/**
 * The public `&self` methods of `impl Core` blocks in `file` whose body
 * does not start in the engine's span (`let _engine = self.engine();`), so
 * what they record would not say which engine recorded it.
 */
export function unnamedMethods(file) {
	// A leading line break lets an `impl` on the first line be found like any other.
	const text = `\n${file}`;
	const found = [];
	const method = /\n {4}pub fn (\w+)(?:<[^>]*>)?\(\s*&self/g;
	for (const match of text.matchAll(method)) {
		const before = text.slice(0, match.index);
		const block = before.lastIndexOf('\nimpl');
		if (block < 0 || !/^\nimpl(?:<[^>]*>)? Core\b/.test(before.slice(block)) || SPAN_OWNERS.has(match[1])) {
			continue;
		}
		const body = text.indexOf('{', match.index + match[0].length);
		const first = text.slice(body + 1).trimStart().split('\n')[0];
		if (first !== 'let _engine = self.engine();') {
			found.push({ name: match[1], line: before.split('\n').length });
		}
	}
	return found;
}

test('a Core method outside the engine span is found, and others are not', () => {
	const text = [
		'impl Core {',
		'    pub fn named(&self) -> u8 {',
		'        let _engine = self.engine();',
		'        1',
		'    }',
		'    pub fn unnamed(',
		'        &self,',
		'        value: u8,',
		'    ) -> u8 {',
		'        value',
		'    }',
		'    pub fn set_trace_name(&self, name: &str) {',
		'        self.shared.span.record("name", name);',
		'    }',
		'}',
		'impl Other {',
		'    pub fn elsewhere(&self) {}',
		'}',
	].join('\n');
	assert.deepEqual(unnamedMethods(text).map(method => method.name), ['unnamed']);
});

test('every public Core method records under its engine', async () => {
	const folder = join(root, 'crates/vnilink/src');
	const entries = await readdir(folder, { recursive: true, withFileTypes: true });
	const offending = [];
	for (const entry of entries.filter(entry => entry.isFile() && entry.name.endsWith('.rs'))) {
		const file = join(entry.parentPath, entry.name);
		for (const method of unnamedMethods(await readFile(file, 'utf8'))) {
			offending.push(`${relative(root, file)}:${String(method.line)}: ${method.name}`);
		}
	}
	assert.deepEqual(offending, [], `start these with \`let _engine = self.engine();\`:\n${offending.join('\n')}`);
});

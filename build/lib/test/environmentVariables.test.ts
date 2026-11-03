// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { environmentPrefixes, environmentVariables } from '../environmentVariables.ts';
import { git } from '../git.ts';

const root = fileURLToPath(new URL('../../../', import.meta.url));

const registry = new Set(['build/lib/environmentVariables.ts', 'build/lib/test/environmentVariables.test.ts']);

/** Each `VNILINK_*` name the tracked files mention, with the files that do. */
async function mentioned(): Promise<Map<string, string[]>> {
	const files = (await git(root, ['ls-files', '-z'])).split('\0').filter(path => path && !registry.has(path) && !path.startsWith('docs/'));
	const names = new Map<string, string[]>();
	for (const path of files) {
		const text = await readFile(join(root, path), 'utf8').catch(() => '');
		for (const [name] of text.matchAll(/\bVNILINK_[A-Z0-9_]+/g)) {
			names.set(name, [...new Set([...names.get(name) ?? [], path])]);
		}
	}
	return names;
}

const names = await mentioned();

suite('environment variables', () => {
	test('every VNILINK_* name the repository mentions is registered', () => {
		const unknown = [...names].filter(([name]) => !(name in environmentVariables) && !environmentPrefixes.includes(name)).map(([name, paths]) => `${name} (${paths.join(', ')})`);
		assert.deepStrictEqual(unknown, [], 'Add each to build/lib/environmentVariables.ts with what it means');
	});
	test('every registered variable is still used', () => {
		assert.deepStrictEqual(Object.keys(environmentVariables).filter(name => !names.has(name)), []);
	});
	test('every prefix names registered variables', () => {
		for (const prefix of environmentPrefixes) {
			assert.ok(Object.keys(environmentVariables).some(name => name.startsWith(prefix)), prefix);
		}
	});
	test('every variable says what it means', () => {
		for (const [name, variable] of Object.entries(environmentVariables)) {
			assert.ok(variable.description.endsWith('.'), `${name} has no description`);
		}
	});
});

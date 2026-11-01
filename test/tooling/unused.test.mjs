// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { checkUnused, findUnused } from '../../build/lib/unused.ts';

async function project(files) {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-unused-'));
	const all = {
		'tsconfig.json': JSON.stringify({ compilerOptions: { strict: true, target: 'ES2024', module: 'NodeNext', moduleResolution: 'NodeNext', noEmit: true, skipLibCheck: true, lib: ['ES2024'], types: [] }, include: ['src/**/*.ts', 'test/**/*.ts'] }),
		'build/lint/unused-allowlist.json': '[]',
		...files,
	};
	for (const [path, content] of Object.entries(all)) {
		await mkdir(dirname(join(root, path)), { recursive: true });
		await writeFile(join(root, path), content);
	}
	return root;
}

const names = report => report.unused.map(entry => `${entry.name}${entry.testOnly ? ' (tests)' : ''}`).sort();

test('reports declarations nothing reaches, including code only unused code uses', async () => {
	const root = await project({
		'src/main.ts': `import { used, Service } from './lib.js';\nused();\nnew Service().run();\n`,
		'src/lib.ts': [
			'export function used(): void { }',
			'export function unused(): void { helper(); }',
			'function helper(): void { }',
			'function recursive(): void { recursive(); }',
			'export class Service { run(): void { } stop(): void { } }',
		].join('\n'),
	});
	try {
		assert.deepEqual(names(findUnused(root)), ['Service.stop', 'helper', 'recursive', 'unused']);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('a member is used through an interface it implements or structurally satisfies', async () => {
	const root = await project({
		'src/main.ts': [
			`import { IDisposable, make, Explicit } from './lib.js';`,
			'const items: IDisposable[] = [make(), new Explicit()];',
			'items.forEach(item => item.dispose());',
		].join('\n'),
		'src/lib.ts': [
			'export interface IDisposable { dispose(): void; }',
			'class Structural { dispose(): void { } }',
			'export function make(): IDisposable { return new Structural(); }',
			'export class Explicit implements IDisposable { dispose(): void { } }',
		].join('\n'),
	});
	try {
		assert.deepEqual(names(findUnused(root)), []);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('reads by key, by `in`, by `keyof` and by destructuring are uses', async () => {
	const root = await project({
		'src/main.ts': [
			`import type { Policy, Mapped, Destructured } from './lib.js';`,
			'declare const policy: Policy; declare const key: \'view\' | \'control\';',
			'export const read = policy[key] && \'files\' in policy;',
			'type Flags = { [K in keyof Mapped]: boolean };',
			'declare const flags: Flags; declare const value: Destructured; const { name } = value; console.log(name, read, flags);',
		].join('\n'),
		'src/lib.ts': [
			'export interface Policy { view: boolean; control: boolean; files: boolean; unread(): void; }',
			'export interface Mapped { a(): void; }',
			'export interface Destructured { name(): void; }',
		].join('\n'),
	});
	try {
		assert.deepEqual(names(findUnused(root)), ['Policy.unread']);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('a declaration only tests reach is reported as such, and allowlisted names must still be needed', async () => {
	const root = await project({
		'src/main.ts': `export {};\n`,
		'src/lib.ts': 'export function forTests(): void { }\nexport function byString(): void { }\n',
		'test/lib.test.ts': `import { forTests } from '../src/lib.js';\nforTests();\n`,
	});
	try {
		assert.deepEqual(names(findUnused(root)), ['byString', 'forTests (tests)']);
		await writeFile(join(root, 'build/lint/unused-allowlist.json'), JSON.stringify([
			{ path: 'src/lib.ts', name: 'byString', reason: 'called by name' },
			{ path: 'src/lib.ts', name: 'forTests', reason: 'test fixture' },
			{ path: 'src/main.ts', name: 'gone', reason: 'stale' },
		]));
		await assert.rejects(checkUnused(root), error => {
			assert.match(error.message, /src\/main\.ts#gone is used now; remove the entry/);
			assert.doesNotMatch(error.message, /byString is unused/);
			return true;
		});
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

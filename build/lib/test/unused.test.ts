// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { after, afterEach, before, suite, test } from 'node:test';
import { checkUnused, findUnused, type UnusedReport } from '../unused.ts';

/** A TypeScript project holding `files` beside an empty allowlist and a manifest naming `src/copied.ts` as copied. */
async function project(files: Readonly<Record<string, string>>): Promise<string> {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-unused-'));
	const all = {
		'tsconfig.json': JSON.stringify({ compilerOptions: { strict: true, target: 'ES2024', module: 'NodeNext', moduleResolution: 'NodeNext', noEmit: true, skipLibCheck: true, lib: ['ES2024'], types: [] }, include: ['src/**/*.ts', 'test/**/*.ts'] }),
		'build/lint/unused-allowlist.json': '[]',
		'build/vscode-source.json': JSON.stringify({ files: [{ path: 'src/copied.ts', upstreamPath: 'src/vs/copied.ts', sha256: '' }], tooling: [] }),
		...files,
	};
	for (const [path, content] of Object.entries(all)) {
		await mkdir(dirname(join(root, path)), { recursive: true });
		await writeFile(join(root, path), content);
	}
	return root;
}

/** The unused names of `report`, sorted, those only tests reach marked `(tests)`. */
const names = (report: UnusedReport): string[] => report.unused.map(entry => `${entry.name}${entry.testOnly ? ' (tests)' : ''}`).sort();

/** The error `promise` rejects with; fails when it resolves. */
async function failureOf(promise: Promise<unknown>): Promise<Error> {
	try {
		await promise;
	} catch (error) {
		assert.ok(error instanceof Error);
		return error;
	}
	assert.fail('expected a rejection');
}

suite('unused', () => {
	const roots: string[] = [];
	const temporary = async (files: Readonly<Record<string, string>>): Promise<string> => {
		const root = await project(files);
		roots.push(root);
		return root;
	};
	afterEach(async () => {
		await Promise.all(roots.splice(0).map(root => rm(root, { recursive: true, force: true })));
	});
	test('reports declarations nothing reaches, code only unused code uses and a class that only constructs itself', async () => {
		const root = await temporary({
			'src/main.ts': `import { used, Service } from './lib.js';\nused();\nnew Service().run();\n`,
			'src/lib.ts': [
				'export function used(): void { }',
				'export function unused(): void { helper(); }',
				'function helper(): void { }',
				'function recursive(): void { recursive(); }',
				'export class Service { run(): void { } stop(): void { } }',
				'class Recursive { make(): Recursive { return new Recursive().helper(); } helper(): Recursive { return this; } }',
			].join('\n'),
		});
		assert.deepStrictEqual(names(findUnused(root)), ['Recursive', 'Service.stop', 'helper', 'recursive', 'unused']);
	});
	test('a member is used through an interface it implements or structurally satisfies', async () => {
		const root = await temporary({
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
		assert.deepStrictEqual(names(findUnused(root)), []);
	});
	test('reads by key, by `in`, by `keyof` and by destructuring are uses', async () => {
		const root = await temporary({
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
		assert.deepStrictEqual(names(findUnused(root)), ['Policy.unread']);
	});
	test('a copied interface\'s unread properties are unused; an application interface\'s are its contract', async () => {
		const root = await temporary({
			'src/main.ts': `import type { Copied } from './copied.js';\nimport type { Contract } from './contract.js';\ndeclare const a: Copied; declare const b: Contract;\nconsole.log(a.read, b.read);\n`,
			'src/copied.ts': 'export interface Copied { read: number; unread: number; }\n',
			'src/contract.ts': 'export interface Contract { read: number; checkedByAGuard: number; }\n',
		});
		assert.deepStrictEqual(names(findUnused(root)), ['Copied.unread']);
	});
	test('a test folder under src is test code: its declarations are not candidates and its uses are by tests', async () => {
		const root = await temporary({
			'src/main.ts': `export {};\n`,
			'src/lib.ts': 'export function forTests(): void { }\n',
			'src/area/test/common/support.ts': `import { forTests } from '../../../lib.js';\nexport function unusedHelper(): void { }\nexport function helper(): void { forTests(); }\n`,
			'src/area/test/common/area.test.ts': `import { helper } from './support.js';\nhelper();\n`,
		});
		assert.deepStrictEqual(names(findUnused(root)), ['forTests (tests)']);
	});
	suite('declarations only tests reach', () => {
		const files = {
			'src/main.ts': `export {};\n`,
			'src/lib.ts': 'export function forTests(): void { }\nexport function byString(): void { }\n',
			'test/lib.test.ts': `import { forTests } from '../src/lib.js';\nforTests();\n`,
		};
		test('reports a declaration only tests reach as such', async () => {
			assert.deepStrictEqual(names(findUnused(await temporary(files))), ['byString', 'forTests (tests)']);
		});
		suite('with an allowlist', () => {
			let root: string;
			let failure: Error;
			before(async () => {
				root = await project({
					...files,
					'build/lint/unused-allowlist.json': JSON.stringify([
						{ path: 'src/lib.ts', name: 'byString', reason: 'called by name' },
						{ path: 'src/lib.ts', name: 'forTests', reason: 'test fixture' },
						{ path: 'src/main.ts', name: 'gone', reason: 'stale' },
					]),
				});
				failure = await failureOf(checkUnused(root));
			});
			after(async () => {
				await rm(root, { recursive: true, force: true });
			});
			test('fails on an entry whose declaration is no longer unused', () => {
				assert.match(failure.message, /src\/main\.ts#gone is used now; remove the entry/);
			});
			test('does not report an allowlisted declaration', () => {
				assert.doesNotMatch(failure.message, /byString is unused/);
			});
		});
	});
	suite('an allowlisted class', () => {
		const files = {
			'src/main.ts': `export {};\n`,
			'src/lib.ts': 'export class Tracker { track(): void { } report(): void { } unused(): void { } }\n',
			'test/lib.test.ts': `import { Tracker } from '../src/lib.js';\nconst tracker = new Tracker(); tracker.track(); tracker.report();\n`,
			'build/lint/unused-allowlist.json': JSON.stringify([{ path: 'src/lib.ts', name: 'Tracker', reason: 'the test leak tracker' }]),
		};
		let root: string;
		let failure: Error;
		before(async () => {
			root = await project(files);
			failure = await failureOf(checkUnused(root));
		});
		after(async () => {
			await rm(root, { recursive: true, force: true });
		});
		test('covers the members only tests use', () => {
			assert.deepStrictEqual(names(findUnused(root, new Set(['src/lib.ts#Tracker']))), ['Tracker.unused']);
		});
		test('leaves the check reporting its unused member', () => {
			assert.match(failure.message, /Tracker\.unused is unused/);
		});
		test('keeps the check from reporting the members only tests use', () => {
			assert.doesNotMatch(failure.message, /Tracker\.(track|report)/);
		});
		test('is not reported as used now', () => {
			assert.doesNotMatch(failure.message, /used now/);
		});
	});
});

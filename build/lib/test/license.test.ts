// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { execFile } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { after, before, suite, test } from 'node:test';
import { applyLicense, checkLicense, findNoticeProblem, noticeKindOf, type ApplyOptions, type NoticeKind } from '../license.ts';
import { commentStyleOf, isCopyrightNotice, leadingMicrosoftNotice, microsoftNotice, modificationNotice, originalNotice, renderNotice, type CommentStyle } from '../licenseNotice.ts';
import { findHygieneViolations, hygieneRules } from '../hygiene.ts';
import { readProduct } from '../product.ts';

const root = fileURLToPath(new URL('../../../', import.meta.url));

const product = await readProduct(root);

const holder = product.publisherName;

const run = promisify(execFile);

// Run from a git hook, GIT_DIR would point these commands at this repository.
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith('GIT_')));

interface CopyEntry {
	readonly path: string;
	readonly adapted?: boolean;
	readonly pruned?: readonly string[];
}

const copies: ReadonlyMap<string, CopyEntry> = new Map<string, CopyEntry>([
	['src/vs/copied.ts', { path: 'src/vs/copied.ts' }],
	['src/vs/changed.ts', { path: 'src/vs/changed.ts', adapted: true }],
	['src/vs/pruned.ts', { path: 'src/vs/pruned.ts', pruned: ['unused'] }],
	['build/lint/vscode/rule.ts', { path: 'build/lint/vscode/rule.ts' }],
]);

const own = (style: CommentStyle): string => renderNotice(style, originalNotice(holder));

/** Why `text` lacks its notice, as a string the assertions can match; `'none'` when nothing is missing. */
function problemOf(path: string, text: string, kind: NoticeKind): string {
	return findNoticeProblem(path, text, kind, holder) ?? 'none';
}

/** A git repository holding the license metadata, a provenance manifest and `files`. */
async function repository(files: Readonly<Record<string, string>>): Promise<string> {
	const directory = await mkdtemp(join(tmpdir(), 'vnilink-license-'));
	const git = (...args: string[]) => run('git', ['-C', directory, '-c', 'user.name=Ada', '-c', 'user.email=ada@example.com', ...args], { env });
	await copyFile(join(root, 'product.json'), join(directory, 'product.json'));
	const base = {
		'build/vscode-source.json': JSON.stringify({ files: [{ path: 'src/vs/copied.ts' }, { path: 'src/vs/changed.ts', adapted: true }], tooling: [] }),
		'package.json': '{ "license": "AGPL-3.0-only" }\n',
		'Cargo.toml': '[workspace.package]\nlicense = "AGPL-3.0-only"\n',
		'LICENSE': '                    GNU AFFERO GENERAL PUBLIC LICENSE\n                       Version 3, 19 November 2007\n',
	};
	for (const [path, text] of Object.entries({ ...base, ...files })) {
		await mkdir(dirname(join(directory, path)), { recursive: true });
		await writeFile(join(directory, path), text);
	}
	await git('init', '--quiet');
	await git('add', '.');
	return directory;
}

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

suite('license', () => {
	suite('notice rendering', () => {
		test('renders the project notice as slash comments', () => {
			assert.strictEqual(renderNotice('slash', originalNotice(holder)), [
				'// ---------------------------------------------------------------------------------------------',
				`// Copyright (c) 2026 ${holder}. All rights reserved.`,
				'// Licensed under the GNU Affero General Public License, version 3 only.',
				'// See LICENSE in the project root for license information.',
				'// SPDX-License-Identifier: AGPL-3.0-only',
				'// ---------------------------------------------------------------------------------------------',
				'',
			].join('\n'));
		});
		test('renders the project notice as hash comments', () => {
			assert.match(renderNotice('hash', originalNotice(holder)), /^# -{93}\n# Copyright/);
		});
		test('renders the project notice as batch remarks', () => {
			assert.match(renderNotice('rem', originalNotice(holder)), /^REM -{93}\nREM Copyright/);
		});
		test('opens a stylesheet notice as a block comment', () => {
			assert.match(renderNotice('css', originalNotice(holder)), /^\/\* -{93}\n \* Copyright/);
		});
		test('closes a stylesheet notice with its rule', () => {
			assert.match(renderNotice('css', originalNotice(holder)), / \* -{93} \*\/\n$/);
		});
		for (const [path, style] of [['crates/a/src/lib.rs', 'slash'], ['src/a.d.ts', 'slash'], ['scripts/test.bat', 'rem'], ['package.json', undefined], ['assets/a.png', undefined], ['LICENSE', undefined]] as const) {
			test(`gives ${path} the ${style ?? 'no'} comment style`, () => {
				assert.strictEqual(commentStyleOf(path), style);
			});
		}
	});
	suite('VS Code notice', () => {
		test('renders the notice a copied TypeScript file starts with', async () => {
			const typescript = await readFile(join(root, 'src/base/browser/browser.ts'), 'utf8');
			assert.ok(typescript.startsWith(microsoftNotice('slash')));
		});
		test('finds the notice a copied TypeScript file starts with', async () => {
			const typescript = await readFile(join(root, 'src/base/browser/browser.ts'), 'utf8');
			assert.strictEqual(leadingMicrosoftNotice(typescript, 'slash'), microsoftNotice('slash'));
		});
		test('renders a copied stylesheet\'s notice with its adaptation lines', async () => {
			const stylesheet = await readFile(join(root, 'src/workbench/parts/titlebar/media/modernTabs.css'), 'utf8');
			const adaptation = stylesheet.split('\n').slice(3, 5).map(line => line.slice(4));
			assert.ok(stylesheet.startsWith(microsoftNotice('css', adaptation)));
		});
		test('finds no notice in code without a comment', () => {
			assert.strictEqual(leadingMicrosoftNotice('const a = 1;\n', 'slash'), undefined);
		});
		test('finds no notice in another holder\'s copyright', () => {
			assert.strictEqual(leadingMicrosoftNotice('/* Copyright (c) someone */\n', 'slash'), undefined);
		});
		test('finds the notice a script starts with', () => {
			assert.strictEqual(leadingMicrosoftNotice(`${microsoftNotice('hash')}\nset -e\n`, 'hash'), microsoftNotice('hash'));
		});
		test('recognises the project notice\'s copyright line', () => {
			assert.ok(isCopyrightNotice(originalNotice(holder)[0]));
		});
		test('recognises the modification notice\'s copyright line', () => {
			assert.ok(isCopyrightNotice(modificationNotice(holder)[0]));
		});
		test('does not take prose about copyright for a copyright line', () => {
			assert.ok(!isCopyrightNotice('Copyright is held by a holder'));
		});
	});
	suite('notice kind', () => {
		const microsoft = `${microsoftNotice('slash')}\ncode();\n`;
		const cases: readonly (readonly [string, string, string, NoticeKind | undefined])[] = [
			['a new file is original', 'src/new.ts', 'code();\n', 'original'],
			['a Rust file is original', 'crates/a/src/lib.rs', '//! A.\n', 'original'],
			['a manifest copy is verbatim', 'src/vs/copied.ts', microsoft, 'verbatim'],
			['an adapted manifest copy is derived', 'src/vs/changed.ts', microsoft, 'derived'],
			// Deleting declarations adds no authorship.
			['a pruned manifest copy is verbatim', 'src/vs/pruned.ts', microsoft, 'verbatim'],
			['a copy outside the manifest is derived by its notice', 'src/moved.ts', microsoft, 'derived'],
			['copied tooling is verbatim without a notice', 'build/lint/vscode/rule.ts', 'code();\n', 'verbatim'],
			// Naming the notice in code does not make a file a copy.
			['code naming VS Code\'s notice is original', 'build/notice.ts', 'const owner = \'Copyright (c) Microsoft Corporation\';\n', 'original'],
			['data has none', 'package.json', '{}\n', undefined],
			['a third-party source has none', 'vendor/iroh/src/lib.rs', 'code();\n', undefined],
			['a generated binding has none', 'crates/a/src/bindings.rs', '//! Generated by bindgen 0.72.1.\n', undefined],
			['an empty file has none', 'src/empty.ts', '', undefined],
		];
		for (const [name, path, text, kind] of cases) {
			test(name, () => {
				assert.strictEqual(noticeKindOf(path, text, copies), kind);
			});
		}
		test('an import of the notice helpers does not count as a notice', () => {
			assert.strictEqual(problemOf('src/a.ts', 'import { isCopyrightNotice } from \'./b.ts\';\n', 'original'), 'has no notice');
		});
	});
	suite('notice problems', () => {
		const ownFile = `${own('slash')}\ncode();\n`;
		const copied = `${microsoftNotice('slash')}\ncode();\n`;
		test('an original file with the project notice has no problem', () => {
			assert.strictEqual(problemOf('src/a.ts', ownFile, 'original'), 'none');
		});
		test('an original file without a notice has no notice', () => {
			assert.strictEqual(problemOf('src/a.ts', 'code();\n', 'original'), 'has no notice');
		});
		test('an original file with another holder\'s notice carries a foreign notice', () => {
			assert.match(problemOf('src/a.ts', '// Copyright (c) 2020 Someone Else\ncode();\n', 'original'), /notice this project did not write/);
		});
		test('an original file with an out-of-date notice carries a foreign notice', () => {
			assert.match(problemOf('src/a.ts', ownFile.replace('2026', '2025'), 'original'), /did not write/);
		});
		test('a derived file without the modification notice lacks it', () => {
			assert.match(problemOf('src/b.ts', copied, 'derived'), /modification notice/);
		});
		test('a derived file with both notices has no problem', () => {
			const changed = `${microsoftNotice('slash')}${renderNotice('slash', modificationNotice(holder))}\ncode();\n`;
			assert.strictEqual(problemOf('src/b.ts', changed, 'derived'), 'none');
		});
		test('a derived file without VS Code\'s notice does not start with it', () => {
			assert.match(problemOf('src/c.ts', 'code();\n', 'derived'), /does not start with its notice/);
		});
		test('a verbatim file with VS Code\'s notice has no problem', () => {
			assert.strictEqual(problemOf('src/d.ts', copied, 'verbatim'), 'none');
		});
		test('a verbatim file with the project notice should keep its own', () => {
			assert.match(problemOf('src/d.ts', ownFile, 'verbatim'), /keeps its own notice/);
		});
	});
	suite('applying the notice', () => {
		const copied = `${microsoftNotice('slash')}\ncopied();\n`;
		const adapted = `${microsoftNotice('slash', ['Adapted for VniLink: nothing.']).replace('See LICENSE.vscode.txt', 'See License.txt')}\nchanged();\n`;
		let directory: string;
		let dryRun: string[];
		let afterDryRun: string;
		let secondRun: string[];
		const recorded: string[] = [];
		const options: ApplyOptions = {
			record: (_root, paths) => {
				recorded.push(...paths);
				return Promise.resolve();
			},
		};
		const read = (path: string) => readFile(join(directory, path), 'utf8');
		before(async () => {
			directory = await repository({
				'src/a.ts': 'import "./b.js";\n',
				'crates/x/src/lib.rs': '//! Docs.\nuse std::fmt;\n',
				'scripts/run.sh': '#!/usr/bin/env bash\nset -e\n',
				'scripts/run.bat': '@echo off\nsetlocal\n',
				'src/ui.css': '.a { color: red; }\n',
				'src/copied.css': `${microsoftNotice('css')}\n.b { color: blue; }\n`,
				'src/vs/copied.ts': copied,
				'src/vs/changed.ts': adapted,
				'data.json': '{}\n',
				'vendor/lib/lib.rs': 'fn main() {}\n',
			});
			dryRun = await applyLicense(directory, options);
			afterDryRun = await read('src/a.ts');
			await applyLicense(directory, { ...options, write: true });
			secondRun = await applyLicense(directory, { ...options, write: true });
		});
		after(async () => {
			await rm(directory, { recursive: true, force: true });
		});
		test('a dry run lists the files that need a notice', () => {
			assert.deepStrictEqual([...dryRun].sort(), ['crates/x/src/lib.rs', 'scripts/run.bat', 'scripts/run.sh', 'src/a.ts', 'src/copied.css', 'src/ui.css', 'src/vs/changed.ts']);
		});
		test('a dry run changes no file', () => {
			assert.strictEqual(afterDryRun, 'import "./b.js";\n');
		});
		test('puts the notice above a TypeScript file\'s code', async () => {
			assert.strictEqual(await read('src/a.ts'), `${own('slash')}\nimport "./b.js";\n`);
		});
		test('puts the notice above a Rust file\'s module documentation', async () => {
			assert.strictEqual(await read('crates/x/src/lib.rs'), `${own('slash')}\n//! Docs.\nuse std::fmt;\n`);
		});
		test('puts the notice below a shebang', async () => {
			assert.strictEqual(await read('scripts/run.sh'), `#!/usr/bin/env bash\n${own('hash')}\nset -e\n`);
		});
		test('puts the notice below a batch file\'s echo off', async () => {
			assert.strictEqual(await read('scripts/run.bat'), `@echo off\n${own('rem')}\nsetlocal\n`);
		});
		test('puts the notice above a stylesheet\'s rules', async () => {
			assert.strictEqual(await read('src/ui.css'), `${own('css')}\n.a { color: red; }\n`);
		});
		test('separates a copied stylesheet\'s modification notice by an empty line', async () => {
			assert.strictEqual(await read('src/copied.css'), `${microsoftNotice('css')}\n${renderNotice('css', modificationNotice(holder))}\n.b { color: blue; }\n`);
		});
		test('leaves a verbatim copy untouched', async () => {
			assert.strictEqual(await read('src/vs/copied.ts'), copied);
		});
		test('corrects an adapted copy\'s VS Code notice and adds the modification notice', async () => {
			assert.strictEqual(await read('src/vs/changed.ts'), `${microsoftNotice('slash', ['Adapted for VniLink: nothing.'])}${renderNotice('slash', modificationNotice(holder))}\nchanged();\n`);
		});
		test('records only the changed copy in the manifest', () => {
			assert.deepStrictEqual(recorded, ['src/vs/changed.ts']);
		});
		test('leaves data untouched', async () => {
			assert.strictEqual(await read('data.json'), '{}\n');
		});
		test('leaves third-party sources untouched', async () => {
			assert.strictEqual(await read('vendor/lib/lib.rs'), 'fn main() {}\n');
		});
		test('changes nothing on a second run', () => {
			assert.deepStrictEqual(secondRun, []);
		});
		test('leaves the repository passing the check', async () => {
			await checkLicense(directory);
		});
	});
	suite('a file that cannot take a notice', () => {
		let directory: string;
		let failure: Error;
		before(async () => {
			directory = await repository({
				'src/a.ts': 'code();\n',
				'src/b.ts': '// Copyright (c) 2019 Someone Else\ncode();\n',
			});
			failure = await failureOf(applyLicense(directory, { write: true, record: () => Promise.resolve() }));
		});
		after(async () => {
			await rm(directory, { recursive: true, force: true });
		});
		test('stops the run saying nothing changed', () => {
			assert.match(failure.message, /Nothing changed/);
		});
		test('names the file and why it cannot take the notice', () => {
			assert.match(failure.message, /src\/b\.ts: original file carries a notice this project did not write/);
		});
		test('does not name the files that could take one', () => {
			assert.doesNotMatch(failure.message, /src\/a\.ts/);
		});
		test('writes nothing', async () => {
			assert.strictEqual(await readFile(join(directory, 'src/a.ts'), 'utf8'), 'code();\n');
		});
	});
	suite('the check', () => {
		let directory: string;
		let failure: Error;
		before(async () => {
			directory = await repository({
				'src/a.ts': 'code();\n',
				'src/vs/copied.ts': `${own('slash')}\ncode();\n`,
				'package.json': '{ "license": "UNLICENSED" }\n',
				'Cargo.toml': '[workspace.package]\nlicense = "Apache-2.0"\n',
				'LICENSE': 'MIT\n',
			});
			failure = await failureOf(checkLicense(directory));
		});
		after(async () => {
			await rm(directory, { recursive: true, force: true });
		});
		const messages: readonly (readonly [string, RegExp])[] = [
			['points to the licensing skill', /\.agents\/skills\/licensing\/SKILL\.md/],
			['names a LICENSE that is not the AGPL', /LICENSE: is not the text of the GNU Affero General Public License, version 3/],
			['names a package.json license that is not the AGPL', /package\.json: "license" is not "AGPL-3\.0-only"/],
			['names a Cargo.toml workspace license that is not the AGPL', /Cargo\.toml: the workspace license is not "AGPL-3\.0-only"/],
			['names an original file without a notice', /src\/a\.ts: original file has no notice/],
			['names a verbatim copy carrying the project notice', /src\/vs\/copied\.ts: verbatim file is copied unchanged/],
		];
		for (const [name, message] of messages) {
			test(name, () => {
				assert.match(failure.message, message);
			});
		}
	});
	suite('hygiene of the notice', () => {
		const rules = hygieneRules(product, []);
		test('allows the project notice\'s copyright line to name the publisher', () => {
			assert.deepStrictEqual(findHygieneViolations('src/a.ts', `// ${originalNotice(holder)[0]}`, rules), []);
		});
		test('allows the modification notice\'s copyright line to name the publisher', () => {
			assert.deepStrictEqual(findHygieneViolations('src/a.ts', ` * ${modificationNotice(holder)[0]}`, rules), []);
		});
		test('finds the publisher named outside a notice', () => {
			assert.strictEqual(findHygieneViolations('src/a.ts', `// Published by ${holder}.`, rules).length, 1);
		});
		test('allows REUSE.toml to name the publisher as the copyright holder', () => {
			assert.deepStrictEqual(findHygieneViolations('REUSE.toml', `SPDX-FileCopyrightText = "${holder}"`, rules), []);
		});
	});
});

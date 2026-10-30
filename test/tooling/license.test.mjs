// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { applyLicense, checkLicense, findNoticeProblem, noticeKindOf } from '../../build/lib/license.ts';
import { commentStyleOf, isCopyrightNotice, leadingMicrosoftNotice, microsoftNotice, modificationNotice, originalNotice, renderNotice } from '../../build/lib/licenseNotice.ts';
import { findHygieneViolations, hygieneRules } from '../../build/lib/hygiene.ts';
import { readProduct } from '../../build/lib/product.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

const product = await readProduct(root);

const holder = product.publisherName;

const run = promisify(execFile);
// Run from a git hook, GIT_DIR would point these commands at this repository.
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith('GIT_')));

const copies = new Map([
	['src/vs/copied.ts', { path: 'src/vs/copied.ts' }],
	['src/vs/changed.ts', { path: 'src/vs/changed.ts', adapted: true }],
]);

test('a file written here carries the project notice in its language\'s comment', () => {
	const lines = originalNotice(holder);
	assert.equal(renderNotice('slash', lines), [
		'// ---------------------------------------------------------------------------------------------',
		`// Copyright (c) 2026 ${holder}. All rights reserved.`,
		'// Licensed under the GNU Affero General Public License, version 3 only.',
		'// See LICENSE in the project root for license information.',
		'// SPDX-License-Identifier: AGPL-3.0-only',
		'// ---------------------------------------------------------------------------------------------',
		'',
	].join('\n'));
	assert.match(renderNotice('hash', lines), /^# -{93}\n# Copyright/);
	assert.match(renderNotice('rem', lines), /^REM -{93}\nREM Copyright/);
	const css = renderNotice('css', lines);
	assert.match(css, /^\/\* -{93}\n \* Copyright/);
	assert.match(css, / \* -{93} \*\/\n$/);
	assert.equal(commentStyleOf('crates/a/src/lib.rs'), 'slash');
	assert.equal(commentStyleOf('src/a.d.ts'), 'slash');
	assert.equal(commentStyleOf('scripts/test.bat'), 'rem');
	assert.equal(commentStyleOf('package.json'), undefined);
	assert.equal(commentStyleOf('assets/a.png'), undefined);
	assert.equal(commentStyleOf('LICENSE'), undefined);
});

test('VS Code\'s notice is rendered as its files carry it', async () => {
	const typescript = await readFile(join(root, 'src/base/browser/browser.ts'), 'utf8');
	assert.ok(typescript.startsWith(microsoftNotice('slash')));
	assert.equal(leadingMicrosoftNotice(typescript, 'slash'), microsoftNotice('slash'));
	const stylesheet = await readFile(join(root, 'src/workbench/parts/titlebar/media/modernTabs.css'), 'utf8');
	const adaptation = stylesheet.split('\n').slice(3, 5).map(line => line.slice(4));
	assert.ok(stylesheet.startsWith(microsoftNotice('css', adaptation)), 'the stylesheet\'s block');
	assert.equal(leadingMicrosoftNotice('const a = 1;\n', 'slash'), undefined);
	assert.equal(leadingMicrosoftNotice('/* Copyright (c) someone */\n', 'slash'), undefined);
	const script = `${microsoftNotice('hash')}\nset -e\n`;
	assert.equal(leadingMicrosoftNotice(script, 'hash'), microsoftNotice('hash'));
	assert.ok(isCopyrightNotice(originalNotice(holder)[0]));
	assert.ok(isCopyrightNotice(modificationNotice(holder)[0]));
	assert.ok(!isCopyrightNotice('Copyright is held by a holder'));
});

test('the notice kind follows the provenance manifest, then the notice a copy already carries', () => {
	const microsoft = `${microsoftNotice('slash')}\ncode();\n`;
	assert.equal(noticeKindOf('src/new.ts', 'code();\n', copies), 'original');
	assert.equal(noticeKindOf('crates/a/src/lib.rs', '//! A.\n', copies), 'original');
	assert.equal(noticeKindOf('src/vs/copied.ts', microsoft, copies), 'verbatim');
	assert.equal(noticeKindOf('src/vs/changed.ts', microsoft, copies), 'derived');
	assert.equal(noticeKindOf('src/moved.ts', microsoft, copies), 'derived');
	assert.equal(noticeKindOf('build/lint/vscode/rule.ts', 'code();\n', copies), 'verbatim');
	// Naming the notice in code does not make a file a copy.
	assert.equal(noticeKindOf('build/notice.ts', "const owner = 'Copyright (c) Microsoft Corporation';\n", copies), 'original');
	assert.equal(findNoticeProblem('src/a.ts', "import { isCopyrightNotice } from './b.ts';\n", 'original', holder), 'has no notice');
	// No notice: data, third-party sources, generated bindings and empty files.
	assert.equal(noticeKindOf('package.json', '{}\n', copies), undefined);
	assert.equal(noticeKindOf('vendor/iroh/src/lib.rs', 'code();\n', copies), undefined);
	assert.equal(noticeKindOf('crates/a/src/bindings.rs', '//! Generated by bindgen 0.72.1.\n', copies), undefined);
	assert.equal(noticeKindOf('src/empty.ts', '', copies), undefined);
});

test('what is missing or foreign is found without changing a file', () => {
	const own = `${renderNotice('slash', originalNotice(holder))}\ncode();\n`;
	assert.equal(findNoticeProblem('src/a.ts', own, 'original', holder), undefined);
	assert.equal(findNoticeProblem('src/a.ts', 'code();\n', 'original', holder), 'has no notice');
	assert.match(findNoticeProblem('src/a.ts', '// Copyright (c) 2020 Someone Else\ncode();\n', 'original', holder), /notice this project did not write/);
	assert.match(findNoticeProblem('src/a.ts', own.replace('2026', '2025'), 'original', holder), /did not write/);
	const copied = `${microsoftNotice('slash')}\ncode();\n`;
	assert.match(findNoticeProblem('src/b.ts', copied, 'derived', holder), /modification notice/);
	const changed = `${microsoftNotice('slash')}${renderNotice('slash', modificationNotice(holder))}\ncode();\n`;
	assert.equal(findNoticeProblem('src/b.ts', changed, 'derived', holder), undefined);
	assert.match(findNoticeProblem('src/c.ts', 'code();\n', 'derived', holder), /does not start with its notice/);
	assert.equal(findNoticeProblem('src/d.ts', copied, 'verbatim', holder), undefined);
	assert.match(findNoticeProblem('src/d.ts', own, 'verbatim', holder), /keeps its own notice/);
});

async function repository(files) {
	const directory = await mkdtemp(join(tmpdir(), 'vnilink-license-'));
	const git = (...args) => run('git', ['-C', directory, '-c', 'user.name=Ada', '-c', 'user.email=ada@example.com', ...args], { env });
	await copyFile(join(root, 'product.json'), join(directory, 'product.json'));
	const base = {
		'build/vscode-source.json': JSON.stringify({ files: [{ path: 'src/vs/copied.ts' }, { path: 'src/vs/changed.ts', adapted: true }] }),
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

test('the notice is added to what lacks one, once, around a shebang or a batch file\'s echo', async () => {
	const copied = `${microsoftNotice('slash')}\ncopied();\n`;
	const adapted = `${microsoftNotice('slash', ['Adapted for VniLink: nothing.']).replace('See LICENSE.vscode.txt', 'See License.txt')}\nchanged();\n`;
	const directory = await repository({
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
	try {
		const recorded = [];
		const options = { record: async paths => void recorded.push(...paths) };
		assert.deepEqual((await applyLicense(directory, options)).sort(), ['crates/x/src/lib.rs', 'scripts/run.bat', 'scripts/run.sh', 'src/a.ts', 'src/copied.css', 'src/ui.css', 'src/vs/changed.ts'], 'a dry run lists the files');
		assert.equal(await readFile(join(directory, 'src/a.ts'), 'utf8'), 'import "./b.js";\n');
		await applyLicense(directory, { ...options, write: true });
		const own = style => renderNotice(style, originalNotice(holder));
		assert.equal(await readFile(join(directory, 'src/a.ts'), 'utf8'), `${own('slash')}\nimport "./b.js";\n`);
		assert.equal(await readFile(join(directory, 'crates/x/src/lib.rs'), 'utf8'), `${own('slash')}\n//! Docs.\nuse std::fmt;\n`);
		assert.equal(await readFile(join(directory, 'scripts/run.sh'), 'utf8'), `#!/usr/bin/env bash\n${own('hash')}\nset -e\n`);
		assert.equal(await readFile(join(directory, 'scripts/run.bat'), 'utf8'), `@echo off\n${own('rem')}\nsetlocal\n`);
		assert.equal(await readFile(join(directory, 'src/ui.css'), 'utf8'), `${own('css')}\n.a { color: red; }\n`);
		// A stylesheet's second comment follows an empty line.
		assert.equal(await readFile(join(directory, 'src/copied.css'), 'utf8'), `${microsoftNotice('css')}\n${renderNotice('css', modificationNotice(holder))}\n.b { color: blue; }\n`);
		// Copied as it is: untouched. Copied and changed: VS Code's notice, corrected, then ours.
		assert.equal(await readFile(join(directory, 'src/vs/copied.ts'), 'utf8'), copied);
		assert.equal(await readFile(join(directory, 'src/vs/changed.ts'), 'utf8'), `${microsoftNotice('slash', ['Adapted for VniLink: nothing.'])}${renderNotice('slash', modificationNotice(holder))}\nchanged();\n`);
		assert.deepEqual(recorded, ['src/vs/changed.ts'], 'only the changed copy is recorded in the manifest');
		assert.equal(await readFile(join(directory, 'data.json'), 'utf8'), '{}\n');
		assert.equal(await readFile(join(directory, 'vendor/lib/lib.rs'), 'utf8'), 'fn main() {}\n');
		assert.deepEqual(await applyLicense(directory, { ...options, write: true }), [], 'a second run changes nothing');
		await checkLicense(directory);
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});

test('a file that cannot take a notice stops the run before anything is written', async () => {
	const directory = await repository({
		'src/a.ts': 'code();\n',
		'src/b.ts': '// Copyright (c) 2019 Someone Else\ncode();\n',
	});
	try {
		await assert.rejects(applyLicense(directory, { write: true, record: async () => undefined }), error => {
			assert.match(error.message, /Nothing changed/);
			assert.match(error.message, /src\/b\.ts: original file carries a notice this project did not write/);
			assert.doesNotMatch(error.message, /src\/a\.ts/);
			return true;
		});
		assert.equal(await readFile(join(directory, 'src/a.ts'), 'utf8'), 'code();\n');
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});

test('the check lists each file and each metadata place that misnames the license', async () => {
	const directory = await repository({
		'src/a.ts': 'code();\n',
		'src/vs/copied.ts': `${renderNotice('slash', originalNotice(holder))}\ncode();\n`,
		'package.json': '{ "license": "UNLICENSED" }\n',
		'Cargo.toml': '[workspace.package]\nlicense = "Apache-2.0"\n',
		'LICENSE': 'MIT\n',
	});
	try {
		await assert.rejects(checkLicense(directory), error => {
			assert.match(error.message, /see \.agents\/skills\/licensing\/SKILL\.md|\.agents\/skills\/licensing\/SKILL\.md/);
			assert.match(error.message, /LICENSE: is not the text of the GNU Affero General Public License, version 3/);
			assert.match(error.message, /package\.json: "license" is not "AGPL-3\.0-only"/);
			assert.match(error.message, /Cargo\.toml: the workspace license is not "AGPL-3\.0-only"/);
			assert.match(error.message, /src\/a\.ts: original file has no notice/);
			assert.match(error.message, /src\/vs\/copied\.ts: verbatim file is copied unchanged/);
			return true;
		});
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});

test('hygiene allows the copyright line of a notice to name the publisher, and nothing else to', () => {
	const rules = hygieneRules(product, []);
	assert.deepEqual(findHygieneViolations('src/a.ts', `// ${originalNotice(holder)[0]}`, rules), []);
	assert.deepEqual(findHygieneViolations('src/a.ts', ` * ${modificationNotice(holder)[0]}`, rules), []);
	assert.equal(findHygieneViolations('src/a.ts', `// Published by ${holder}.`, rules).length, 1);
	assert.deepEqual(findHygieneViolations('REUSE.toml', `SPDX-FileCopyrightText = "${holder}"`, rules), []);
});

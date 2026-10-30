// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { checkHygiene, findCommentViolations, findHygieneViolations, hygieneRules, productIdentity } from '../../build/lib/hygiene.ts';
import { readProduct } from '../../build/lib/product.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

const product = await readProduct(root);

const run = promisify(execFile);
// Run from a git hook, GIT_DIR would point these commands at this repository.
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith('GIT_')));

// Seeds are assembled so this file names no home directory or handle itself.
const home = (...segments) => segments.join('/');

const author = `Ada Lovelace\0${['ada', 'lovelace'].join('.')}@example.com`;

const rules = hygieneRules(product, [author, `Ada Lovelace\0${'123+analytical'}@users.noreply.github.com`]);

test('home directories other than the placeholders are found, in every path form', () => {
	const seeded = [
		`open ${home('', 'Users', 'jane', 'Projects')}`,
		`'0:${home('Users', 'jane', 'Downloads')}'`,
		`'C:${'\\\\'}Users${'\\\\'}jane${'\\\\'}AppData'`,
		`--remote-root ${home('', 'home', 'jane', 'vnilink')}`,
	];
	for (const line of seeded) {
		const findings = findHygieneViolations('test/a.ts', line, rules);
		assert.equal(findings.length, 1, line);
		assert.match(findings[0].message, /home directory .*jane.*: use \/Users\/someone/);
	}
	assert.deepEqual(findHygieneViolations('test/a.ts', `x\n\n${seeded[0]}`, rules).map(finding => [finding.path, finding.line]), [['test/a.ts', 3]]);
});

test('placeholders, shared homes and paths that are not homes pass', () => {
	const clean = [
		home('', 'Users', 'someone', 'Secret'),
		'C:\\Users\\someone\\doc.pdf',
		home('', 'home', 'someone', '.local'),
		home('', 'Users', 'Shared'),
		`import { HomeView } from '../${home('home', 'homeView.js')}';`,
		home('src', 'contrib', 'home', 'homePages.ts'),
		`'${home('Users', '..', 'etc')}'`,
		'"C:\\Users\\..." instead of "file:///c%3A/Users/..."',
		'mount /home rw',
	];
	assert.deepEqual(clean.flatMap(line => findHygieneViolations('src/a.ts', line, rules)), []);
});

test('git authors\' name words and email handles are found as whole words', () => {
	assert.deepEqual(new Set(rules.handles), new Set(['lovelace', 'analytical']));
	assert.match(findHygieneViolations('README.md', 'Confirmed by Lovelace.', rules)[0].message, /author's handle "Lovelace"/);
	assert.equal(findHygieneViolations('README.md', 'the analytical engine', rules).length, 1);
	assert.deepEqual(findHygieneViolations('README.md', 'lovelaces, ada and 123', rules), []);
});

test('product identity is allowed in product.json and the contact documents only', () => {
	const identity = productIdentity(product);
	assert.ok(identity.includes(new URL(product.repositoryUrl).pathname.split('/')[1].toLowerCase()));
	assert.ok(identity.includes(product.supportEmail.split('@')[1]));
	for (const line of [product.appId, product.supportEmail, product.publisherName.toUpperCase(), product.repositoryUrl]) {
		const findings = findHygieneViolations('test/tooling/a.test.mjs', `assert.equal(id, '${line}');`, rules);
		assert.equal(findings.length, 1, line);
		assert.match(findings[0].message, /product identity .*: read it from product\.json/);
		assert.deepEqual(findHygieneViolations('SECURITY.md', line, rules), []);
	}
});

test('the check fails on a seeded repository, listing each finding, and passes once it is clean', async () => {
	const directory = await mkdtemp(join(tmpdir(), 'vnilink-hygiene-'));
	const git = (...args) => run('git', ['-C', directory, '-c', 'user.name=Ada Lovelace', '-c', `user.email=${['ada', 'lovelace'].join('.')}@example.com`, ...args], { env });
	try {
		await copyFile(join(root, 'product.json'), join(directory, 'product.json'));
		await mkdir(join(directory, 'vendor'));
		await mkdir(join(directory, 'build'));
		await writeFile(join(directory, 'build', 'vscode-source.json'), '{"files":[]}');
		await writeFile(join(directory, 'vendor', 'notes.md'), home('', 'Users', 'jane'));
		await writeFile(join(directory, 'README.md'), 'VniLink.\n');
		await git('init', '--quiet');
		await git('add', '.');
		await git('commit', '--quiet', '-m', 'start');
		await checkHygiene(directory);
		await writeFile(join(directory, 'README.md'), `VniLink.\nBuilt in ${home('', 'Users', 'jane')} by Lovelace.\n`);
		await writeFile(join(directory, 'new.ts'), `const id = '${product.appId}';\n`);
		await writeFile(join(directory, 'cited.rs'), '// Licensed under the project.\n\n//! Rules are in `PAIRING.md`.\n');
		await assert.rejects(checkHygiene(directory), error => {
			assert.match(error.message, /see "Writing about the project" in the error-doc-voice skill/);
			assert.match(error.message, /cited\.rs:3: comment cites a markdown document "PAIRING\.md"/);
			assert.match(error.message, /README\.md:2: home directory/);
			assert.match(error.message, /README\.md:2: git author's handle "Lovelace"/);
			assert.match(error.message, /new\.ts:1: product identity/);
			assert.doesNotMatch(error.message, /vendor/);
			return true;
		});
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});

test('comments name code, never a document, a skill or a plan', () => {
	const cited = [
		['//! The keyboard steps are in `crates/vnilink/KEYBOARD.md`.', /markdown document "crates\/vnilink\/KEYBOARD\.md"/],
		['/// Session lifecycle from the README.', /project document "README"/],
		['\t// Written as .agents/skills/error-doc-voice asks.', /skill or documentation folder/],
		[' * See docs/architecture/files.', /skill or documentation folder/],
		[`let x = 1; // done in ${'phase'} 2`, /cites a plan/],
	];
	for (const [line, message] of cited) {
		const findings = findCommentViolations('src/a.ts', line);
		assert.equal(findings.length, 1, line);
		assert.match(findings[0].message, message);
	}
	for (const line of [`// ${'This'} laptop's camera: a 4:3 sensor.`, `/// A pause that took 0.8 s ${'here'} (2026-10-23).`, `// Measured with an ${'RTX'} 5060.`, `// On an ${'Apple'} M1.`]) {
		const findings = findCommentViolations('src/a.rs', line);
		assert.equal(findings.length, 1, line);
		assert.match(findings[0].message, /one contributor's computer|one GPU or chip model/);
	}
	const clean = [
		'// A hybrid laptop whose panel is on the integrated GPU (2026-10-22).',
		'// On Apple silicon the encoder is hardware.',
		'const url = "https://example.com/README"; ',
		'// Reads the stage 1 output of `decode` and the step size.',
		'let value = *pointer * 2; // the number of steps',
		'//! Pairing: see [`Core::confirm_control_request`].',
		'const name = "LATENCY.md";',
		'let readme = 1;',
	];
	assert.deepEqual(clean.flatMap(line => findCommentViolations('src/a.ts', line)), []);
	assert.deepEqual(findCommentViolations('src/a.ts', 'x\n\n// see CONTRIBUTING').map(finding => finding.line), [3]);
});

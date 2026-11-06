// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { execFile } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { after, before, suite, test } from 'node:test';
import { checkHygiene, findCommentViolations, findHygieneViolations, hygieneRules, productIdentity } from '../hygiene.ts';
import { isWindows } from '../platform.ts';
import { readProduct } from '../product.ts';

const root = fileURLToPath(new URL('../../../', import.meta.url));

const product = await readProduct(root);

const run = promisify(execFile);

// Run from a git hook, GIT_DIR would point these commands at this repository.
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith('GIT_')));

// Seeds are assembled so this file names no home directory or handle itself.
const home = (...segments: string[]): string => segments.join('/');

const email = `${['ada', 'lovelace'].join('.')}@example.com`;

/** A git author line, as `git log` prints name and email. */
const author = (name: string, address: string): string => `${name}\0${address}`;

const rules = hygieneRules(product, [author('Ada Lovelace', email), author('Ada Lovelace', '123+analytical@users.noreply.github.com')]);

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

suite('hygiene', () => {
	suite('home directories', () => {
		const seeded: readonly (readonly [string, string])[] = [
			['a macOS path', `open ${home('', 'Users', 'jane', 'Projects')}`],
			['a root-relative path', `'0:${home('Users', 'jane', 'Downloads')}'`],
			['a Windows path', `'${['C:', 'Users', 'jane', 'AppData'].join('\\\\')}'`],
			['a Linux path', `--remote-root ${home('', 'home', 'jane', 'vnilink')}`],
		];
		for (const [form, line] of seeded) {
			test(`finds a home directory in ${form}`, () => {
				const findings = findHygieneViolations('test/a.ts', line, rules);
				assert.strictEqual(findings.length, 1);
				assert.match(findings[0].message, /home directory .*jane.*: use \/Users\/someone/);
			});
		}
		test('reports the path and line of a finding', () => {
			assert.deepStrictEqual(findHygieneViolations('test/a.ts', `x\n\n${seeded[0][1]}`, rules).map(finding => [finding.path, finding.line]), [['test/a.ts', 3]]);
		});
		const clean: readonly (readonly [string, string])[] = [
			['the macOS placeholder', home('', 'Users', 'someone', 'Secret')],
			['the Windows placeholder', 'C:\\Users\\someone\\doc.pdf'],
			['the Linux placeholder', home('', 'home', 'someone', '.local')],
			['the shared macOS home', home('', 'Users', 'Shared')],
			['an import from a home folder of the source', `import { HomeView } from '../${home('home', 'homeView.js')}';`],
			['a source path through a home folder', home('src', 'contrib', 'home', 'homePages.ts')],
			['a parent reference under Users', `'${home('Users', '..', 'etc')}'`],
			['elided Windows and URI paths', '"C:\\Users\\..." instead of "file:///c%3A/Users/..."'],
			['the home mount point', 'mount /home rw'],
		];
		for (const [name, line] of clean) {
			test(`passes ${name}`, () => {
				assert.deepStrictEqual(findHygieneViolations('src/a.ts', line, rules), []);
			});
		}
	});
	suite('git authors', () => {
		test('takes the handles from authors\' name words and email handles', () => {
			assert.deepStrictEqual(new Set(rules.handles), new Set(['lovelace', 'analytical']));
		});
		test('finds a name word whatever its case', () => {
			assert.match(findHygieneViolations('README.md', 'Confirmed by Lovelace.', rules)[0].message, /author's handle "Lovelace"/);
		});
		test('finds an email handle', () => {
			assert.strictEqual(findHygieneViolations('README.md', 'the analytical engine', rules).length, 1);
		});
		test('finds handles only as whole words of four letters or more', () => {
			assert.deepStrictEqual(findHygieneViolations('README.md', 'lovelaces, ada and 123', rules), []);
		});
	});
	suite('product identity', () => {
		test('includes the repository owner', () => {
			assert.ok(productIdentity(product).includes(new URL(product.repositoryUrl).pathname.split('/')[1].toLowerCase()));
		});
		test('includes the support mail domain', () => {
			assert.ok(productIdentity(product).includes(product.supportEmail.split('@')[1]));
		});
		const identities: readonly (readonly [string, string])[] = [
			['application id', product.appId],
			['support email', product.supportEmail],
			['publisher name in capitals', product.publisherName.toUpperCase()],
			['repository URL', product.repositoryUrl],
		];
		for (const [name, value] of identities) {
			test(`finds the ${name} in source`, () => {
				const findings = findHygieneViolations('build/lib/test/a.test.ts', `assert.equal(id, '${value}');`, rules);
				assert.strictEqual(findings.length, 1);
				assert.match(findings[0].message, /product identity .*: read it from product\.json/);
			});
			test(`allows the ${name} in the contact documents`, () => {
				assert.deepStrictEqual(findHygieneViolations('SECURITY.md', value, rules), []);
				assert.deepStrictEqual(findHygieneViolations('README.md', value, rules), []);
			});
		}
	});
	suite('the check', () => {
		let directory: string;
		let clean: Error | undefined;
		let failure: Error;
		before(async () => {
			directory = await mkdtemp(join(tmpdir(), 'vnilink-hygiene-'));
			const git = (...args: string[]) => run('git', ['-C', directory, '-c', 'user.name=Ada Lovelace', '-c', `user.email=${email}`, ...args], { env });
			await copyFile(join(root, 'product.json'), join(directory, 'product.json'));
			await mkdir(join(directory, 'vendor'));
			await mkdir(join(directory, 'build'));
			await writeFile(join(directory, 'build', 'vscode-source.json'), '{"files":[],"tooling":[]}');
			await writeFile(join(directory, 'vendor', 'notes.md'), home('', 'Users', 'jane'));
			await writeFile(join(directory, 'README.md'), 'VniLink.\n');
			// A link to a folder whose files git lists under the folder; Windows checks links out as files.
			if (!isWindows) {
				await mkdir(join(directory, 'skills'));
				await writeFile(join(directory, 'skills', 'notes.md'), 'Notes.\n');
				await symlink('skills', join(directory, 'linked'));
			}
			await git('init', '--quiet');
			await git('add', '.');
			await git('commit', '--quiet', '-m', 'start');
			clean = await checkHygiene(directory).then(() => undefined, (error: unknown) => error instanceof Error ? error : new Error(String(error)));
			await writeFile(join(directory, 'README.md'), `VniLink.\nBuilt in ${home('', 'Users', 'jane')} by Lovelace.\n`);
			await writeFile(join(directory, 'new.ts'), `const id = '${product.appId}';\n`);
			await writeFile(join(directory, 'cited.rs'), '// Licensed under the project.\n\n//! Rules are in `PAIRING.md`.\n');
			failure = await failureOf(checkHygiene(directory));
		});
		after(async () => {
			await rm(directory, { recursive: true, force: true });
		});
		test('passes a clean repository', () => {
			assert.strictEqual(clean, undefined);
		});
		const messages: readonly (readonly [string, RegExp])[] = [
			['points to the error-doc-voice skill', /see "Writing about the project" in the error-doc-voice skill/],
			['lists a comment citing a document', /cited\.rs:3: comment cites a markdown document "PAIRING\.md"/],
			['lists a home directory', /README\.md:2: home directory/],
			['lists a git author\'s handle', /README\.md:2: git author's handle "Lovelace"/],
			['lists product identity in an untracked file', /new\.ts:1: product identity/],
		];
		for (const [name, message] of messages) {
			test(name, () => {
				assert.match(failure.message, message);
			});
		}
		test('skips third-party sources', () => {
			assert.doesNotMatch(failure.message, /vendor/);
		});
	});
	suite('comments', () => {
		const cited: readonly (readonly [string, string, RegExp])[] = [
			['a markdown document', '//! The keyboard steps are in `crates/vnilink/KEYBOARD.md`.', /markdown document "crates\/vnilink\/KEYBOARD\.md"/],
			['a project document', '/// Session lifecycle from the README.', /project document "README"/],
			['a skill', '\t// Written as .agents/skills/error-doc-voice asks.', /skill or documentation folder/],
			['a documentation folder', ' * See docs/architecture/files.', /skill or documentation folder/],
			['a plan', ['let x = 1; // done in', 'phase', '2'].join(' '), /cites a plan/],
		];
		for (const [name, line, message] of cited) {
			test(`finds a comment citing ${name}`, () => {
				const findings = findCommentViolations('src/a.ts', line);
				assert.strictEqual(findings.length, 1);
				assert.match(findings[0].message, message);
			});
		}
		const machines: readonly (readonly [string, string])[] = [
			['a contributor\'s device', ['//', 'This', 'laptop\'s camera: a 4:3 sensor.'].join(' ')],
			['a measurement taken on one computer', ['/// A pause that took 0.8 s', 'here', '(2026-10-23).'].join(' ')],
			['a GPU model', ['// Measured with an', 'RTX', '5060.'].join(' ')],
			['a chip model', ['// On an', 'Apple', 'M1.'].join(' ')],
		];
		for (const [name, line] of machines) {
			test(`finds a comment describing ${name}`, () => {
				const findings = findCommentViolations('src/a.rs', line);
				assert.strictEqual(findings.length, 1);
				assert.match(findings[0].message, /one contributor's computer|one GPU or chip model/);
			});
		}
		const clean: readonly (readonly [string, string])[] = [
			['a class of hardware with a date', '// A hybrid laptop whose panel is on the integrated GPU (2026-10-22).'],
			['a platform family', '// On Apple silicon the encoder is hardware.'],
			['a URL ending in a document name', 'const url = "https://example.com/README"; '],
			['a numbered stage of code', '// Reads the stage 1 output of `decode` and the step size.'],
			['a comment after a dereference', 'let value = *pointer * 2; // the number of steps'],
			['a code reference', '//! Pairing: see [`Core::confirm_control_request`].'],
			['a document name in a string', 'const name = "LATENCY.md";'],
			['an identifier named after a document', 'let readme = 1;'],
		];
		for (const [name, line] of clean) {
			test(`passes ${name}`, () => {
				assert.deepStrictEqual(findCommentViolations('src/a.ts', line), []);
			});
		}
		test('reports the line of a comment finding', () => {
			assert.deepStrictEqual(findCommentViolations('src/a.ts', 'x\n\n// see CONTRIBUTING').map(finding => finding.line), [3]);
		});
	});
});

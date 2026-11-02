// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { RuleTester, type Rule } from 'eslint';
import { suite, test } from 'node:test';
import tseslint from 'typescript-eslint';
import { importExtensionsRule } from '../../lint/imports.ts';
import { synchronousFsRule } from '../../lint/synchronousFs.ts';

RuleTester.describe = suite;

RuleTester.it = test;

// typescript-eslint types a rule's context more richly than ESLint's RuleTester does; the rule object is the one ESLint runs.
const tester = new RuleTester({ languageOptions: { parser: tseslint.parser } });

tester.run('import-extensions', importExtensionsRule as unknown as Rule.RuleModule, {
	valid: ["import metadata from './product.json' with { type: 'json' };", "import './module.js';", "import './style.css';", "import type { X } from 'electron';", "type X = import('./module.js').X;"],
	invalid: [
		"import metadata from './product.json';", "const data = import('./product.json');", "import './module';", "export * from './module.ts';", "export { x } from './module';", "const value = import('./module');", "type X = import('./module').X;",
	].map(code => ({ code, errors: [{ messageId: 'extension' }] })),
});

tester.run('no-sync-fs', synchronousFsRule as unknown as Rule.RuleModule, {
	valid: ["import { readFile } from 'node:fs'; readFile('file', () => {});", "import * as fs from 'node:fs'; function f(fs) { fs.readFileSync('file'); }"],
	invalid: [
		"import { readFileSync as read } from 'node:fs'; read('file');",
		"import * as disk from 'fs'; disk.readFileSync('file');",
		"import disk from 'node:fs'; disk['statSync']('file');",
	].map(code => ({ code, errors: [{ messageId: 'synchronous' }] })),
});

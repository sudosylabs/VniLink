// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { RuleTester, type Rule } from 'eslint';
import { suite, test } from 'node:test';
import tseslint from 'typescript-eslint';
import { buildClassesRule } from '../../lint/buildClasses.ts';

RuleTester.describe = suite;

RuleTester.it = test;

// typescript-eslint types a rule's context more richly than ESLint's RuleTester does; the rule object is the one ESLint runs.
const tester = new RuleTester({ languageOptions: { parser: tseslint.parser } });

const allow = [{ allow: ['build/lib/processes.ts'] }];

tester.run('vnilink/build-classes', buildClassesRule as unknown as Rule.RuleModule, {
	valid: [
		{ code: 'export async function buildRelease(root, signal) { }', filename: '/repo/build/lib/release.ts', options: allow },
		{ code: 'export const bundle = define("bundle", async () => { });', filename: '/repo/build/lib/bundles.ts', options: allow },
		{ code: 'export class ProcessRunner { }', filename: '/repo/build/lib/processes.ts', options: allow },
	],
	invalid: [
		{ code: 'export class ReleaseBuild { async run() { } }', filename: '/repo/build/lib/release.ts', options: allow, errors: [{ messageId: 'class' }] },
		{ code: 'const Task = class { };', filename: '/repo/build/lib/task.ts', options: allow, errors: [{ messageId: 'class' }] },
	],
});

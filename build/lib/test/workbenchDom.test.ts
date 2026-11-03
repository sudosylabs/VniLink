// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { RuleTester, type Rule } from 'eslint';
import { suite, test } from 'node:test';
import tseslint from 'typescript-eslint';
import { workbenchDomRule } from '../../lint/workbenchDom.ts';

RuleTester.describe = suite;

RuleTester.it = test;

// typescript-eslint types a rule's context more richly than ESLint's RuleTester does; the rule object is the one ESLint runs.
const tester = new RuleTester({ languageOptions: { parser: tseslint.parser } });

tester.run('vnilink/workbench-dom', workbenchDomRule as unknown as Rule.RuleModule, {
	valid: [
		{ code: "const row = append(parent, $('.device-row'));" },
		{ code: "button.addEventListener('click', run);" },
		{ code: 'this.actionBar.push(actions, { icon: true });' },
		{ code: 'element.click = undefined;' },
	],
	invalid: [
		{ code: "const row = document.createElement('div');", errors: [{ messageId: 'createElement' }] },
		{ code: 'this.pairing.importButton.click();', errors: [{ messageId: 'click' }] },
	],
});

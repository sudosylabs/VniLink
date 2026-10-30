// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { ESLint } from 'eslint';

// Proves the ported VS Code rules are wired into the repository configuration, not just present on disk.
// The sample is linted under a real file's path. With CI=true typescript-eslint infers a single run and
// builds its program from the file on disk instead of this text, so that inference is turned off here.
const eslint = new ESLint({
	cwd: fileURLToPath(new URL('../../', import.meta.url)),
	overrideConfig: [{ files: ['**/*.ts'], languageOptions: { parserOptions: { disallowAutomaticSingleRunInference: true } } }],
});

async function rulesFor(code) {
	const [result] = await eslint.lintText(code, { filePath: 'src/workbench/workbench.ts' });
	return result.messages.map(message => message.ruleId);
}

test('ported VS Code rules report their violations', async () => {
	const rules = await rulesFor(`import { Disposable, DisposableStore } from '../../base/common/lifecycle.js';
import * as nls from '../../nls.js';

export class SampleService extends Disposable {
	public readonly _serviceBrand = undefined;
	public store = new DisposableStore();

	public label(id: string): string {
		return nls.localize(\`sample.\${id}\`, 'Sample');
	}

	public override dispose(): void {
		this.store.clear();
	}
}
`);
	for (const rule of ['local/code-declare-service-brand', 'local/code-no-potentially-unsafe-disposables', 'local/code-no-localization-template-literals', 'local/code-must-use-super-dispose']) {
		assert.ok(rules.includes(rule), `${rule} did not report: ${JSON.stringify(rules)}`);
	}
});

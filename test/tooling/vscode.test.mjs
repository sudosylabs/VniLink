// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { flattenSource, localPath } from '../../build/lib/vscode/manifest.ts';
import { pruneSource } from '../../build/lib/vscode/prune.ts';

test('upstream paths map into src without the vs segment', () => {
	assert.equal(localPath('src/vs/base/browser/ui/list/listWidget.ts'), 'src/base/browser/ui/list/listWidget.ts');
	assert.throws(() => localPath('extensions/git/src/main.ts'));
});

test('copies keep upstream text apart from the declared license and import flattening', () => {
	const source = ` *  Licensed under the MIT License. See License.txt in the project root for license information.
// see vs/base/common/event.ts and 'vs/platform/log/common/log.js'
// a repository path stays: src/vs/workbench/browser/workbench.ts
const keep = 'avs/base/x';`;
	assert.equal(flattenSource(source), ` *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
// see base/common/event.ts and 'platform/log/common/log.js'
// a repository path stays: src/vs/workbench/browser/workbench.ts
const keep = 'avs/base/x';`);
});

const upstream = `import { a, b, c } from './x.js';
import { neverUsed } from './y.js';
import { onlyInOne } from './z.js';

// A section comment stays with the code after it.

/**
 * Documents one.
 */
export function one(): number {
	return b + onlyInOne;
}

export function two(): number {
	return a + c;
}

export class MenuId {

	static readonly First = new MenuId('a');
	static readonly Second = new MenuId('b'); // trailing

	/** Documents third. */
	static readonly Third = new MenuId('c');

	constructor(readonly id: string) { }
}

export { two as three, one as four };
`;

test('pruning deletes whole declarations with their comments and the imports only they used', () => {
	assert.equal(pruneSource('a.ts', upstream, ['one', 'MenuId.First', 'MenuId.Second', 'four']), `import { a, c } from './x.js';
import { neverUsed } from './y.js';

// A section comment stays with the code after it.

export function two(): number {
	return a + c;
}

export class MenuId {

	/** Documents third. */
	static readonly Third = new MenuId('c');

	constructor(readonly id: string) { }
}

export { two as three };
`, 'an import upstream left unused stays; the blank line under the class brace stays single');
});

test('pruning the last member of a block leaves no blank line against its brace', () => {
	const pruned = pruneSource('a.ts', upstream, ['MenuId.Third', 'two', 'three']);
	assert.match(pruned, /static readonly Second = new MenuId\('b'\); \/\/ trailing\n\n\tconstructor/);
	assert.match(pruned, /import \{ b \} from '\.\/x\.js';/);
	assert.match(pruned, /export \{ one as four \};\n$/);
});

test('pruning refuses a name it cannot find', () => {
	assert.throws(() => pruneSource('a.ts', upstream, ['missing', 'MenuId.Gone']), /no declaration named missing, MenuId\.Gone/);
});

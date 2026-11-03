// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { suite, test } from 'node:test';
import { flattenSource, localPath } from '../vscode/manifest.ts';
import { pruneSource } from '../vscode/prune.ts';

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

suite('vscode', () => {
	suite('import', () => {
		test('maps an upstream path into src without the vs segment', () => {
			assert.strictEqual(localPath('src/vs/base/browser/ui/list/listWidget.ts'), 'src/base/browser/ui/list/listWidget.ts');
		});
		test('refuses an upstream path outside src/vs', () => {
			assert.throws(() => localPath('extensions/git/src/main.ts'));
		});
		test('keeps upstream text apart from the declared license and import flattening', () => {
			const source = ` *  Licensed under the MIT License. See License.txt in the project root for license information.
// see vs/base/common/event.ts and 'vs/platform/log/common/log.js'
// a repository path stays: src/vs/workbench/browser/workbench.ts
const keep = 'avs/base/x';`;
			assert.strictEqual(flattenSource(source), ` *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
// see base/common/event.ts and 'platform/log/common/log.js'
// a repository path stays: src/vs/workbench/browser/workbench.ts
const keep = 'avs/base/x';`);
		});
	});
	suite('pruning', () => {
		// An import upstream left unused stays; the blank line under the class brace stays single.
		test('deletes whole declarations with their comments and the imports only they used', () => {
			assert.strictEqual(pruneSource('a.ts', upstream, ['one', 'MenuId.First', 'MenuId.Second', 'four']), `import { a, c } from './x.js';
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
`);
		});
		test('leaves no blank line against the brace when the last member of a block goes', () => {
			assert.match(pruneSource('a.ts', upstream, ['MenuId.Third', 'two', 'three']), /static readonly Second = new MenuId\('b'\); \/\/ trailing\n\n\tconstructor/);
		});
		test('keeps the imported names the remaining code uses', () => {
			assert.match(pruneSource('a.ts', upstream, ['MenuId.Third', 'two', 'three']), /import \{ b \} from '\.\/x\.js';/);
		});
		test('keeps the remaining names of an export list', () => {
			assert.match(pruneSource('a.ts', upstream, ['MenuId.Third', 'two', 'three']), /export \{ one as four \};\n$/);
		});
		test('refuses names it cannot find', () => {
			assert.throws(() => pruneSource('a.ts', upstream, ['missing', 'MenuId.Gone']), /no declaration named missing, MenuId\.Gone/);
		});
		test('keeps the file notice when the first import goes, and removes widened neighbours once', () => {
			const notice = `/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *--------------------------------------------------------------------------------------------*/
`;
			const source = `${notice}import { gone } from './gone.js';
import { kept } from './kept.js';

export class Queue {
	peek(): number {
		return kept;
	}

	removeLast(): number {
		return gone;
	}

	takeCount(): number {
		return gone;
	}
}
`;
			assert.strictEqual(pruneSource('a.ts', source, ['Queue.removeLast', 'Queue.takeCount']), `${notice}import { kept } from './kept.js';

export class Queue {
	peek(): number {
		return kept;
	}
}
`);
		});
		test('keeps upstream\'s trailing comma when the last imported names go', () => {
			const source = `import { a, b, c, } from './x.js';\n\nexport const kept = a;\nexport const gone = b + c;\n`;
			assert.strictEqual(pruneSource('a.ts', source, ['gone']), `import { a, } from './x.js';\n\nexport const kept = a;\n`);
		});
		test('reaches into namespaces, and a removed owner takes the names declared inside it', () => {
			const source = `export namespace Event {
	export function map(): void { }

	export class Chain {
		reduce(): void { }

		latch(): void { }
	}
}

export class Reader {
	peek(): void { }
}
`;
			assert.strictEqual(pruneSource('a.ts', source, ['Event.Chain.reduce', 'Reader', 'Reader.peek']), `export namespace Event {
	export function map(): void { }

	export class Chain {
		latch(): void { }
	}
}
`);
		});
		test('keeps a table\'s layout and trailing comma when its entries go', () => {
			const source = `export const icons = {
	add: register('add', 1),
	check: register('check', 2),
	gone: register('gone', 3),
} as const;
`;
			assert.strictEqual(pruneSource('a.ts', source, ['icons.add', 'icons.gone']), `export const icons = {
	check: register('check', 2),
} as const;
`);
		});
		test('keeps the comments of a table\'s remaining entries, and a missing trailing comma missing', () => {
			const source = `export const types = {
	/**
	 * Gone.
	 */
	GONE: 'gone',

	/**
	 * Kept.
	 */
	KEPT: 'kept',
	LAST: 'last'
};
`;
			assert.strictEqual(pruneSource('a.ts', source, ['types.GONE', 'types.LAST']), `export const types = {
	/**
	 * Kept.
	 */
	KEPT: 'kept'
};
`);
		});
	});
});

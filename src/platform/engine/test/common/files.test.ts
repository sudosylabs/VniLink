// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isFileOperation } from '../../common/files.js';

suite('isFileOperation', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const accepted: [string, unknown][] = [
		['a listing under a root', { type: 'list', root: '0', path: 'Users/someone', offset: '0' }],
		['a later page of a root', { type: 'list', root: '2', path: '', offset: '500' }],
		['a rename with its identity token', { type: 'rename', root: '0', path: '', name: 'a', token: { kind: 'identity', dev: '1', ino: null, born: '7', fingerprint: null }, newName: 'b' }],
		['a copy of entries of a whole disk', { type: 'submit', direction: 'upload', srcRoot: '0', srcPath: '', names: ['a.txt', 'Photos'], dstRoot: '0', dstPath: 'x' }],
		['a conflict resolved for the rest', { type: 'resolve', jobId: '3', choice: 'keepBoth', applyToRemaining: true }],
		['adopting interrupted jobs', { type: 'adopt' }],
		['watching a folder', { type: 'watch', folder: { root: '2', path: 'Users/someone/Downloads' } }],
		['watching nothing', { type: 'watch', folder: null }],
	];
	for (const [name, operation] of accepted) {
		test(`${name} is accepted`, () => {
			assert.strictEqual(isFileOperation(operation), true);
		});
	}
	const refused: [string, unknown][] = [
		['a path with a leading slash', { type: 'list', root: '0', path: '/Users', offset: '0' }],
		['a path with a parent step', { type: 'list', root: '0', path: 'Users/../etc', offset: '0' }],
		['a name with a separator', { type: 'mkdir', root: '0', path: '', name: 'a/b' }],
		['a rename without a token field', { type: 'rename', root: '0', path: '', name: 'a', newName: 'b' }],
		['a copy of a whole root', { type: 'submit', direction: 'upload', srcRoot: '0', srcPath: '', names: [], dstRoot: '0', dstPath: 'x' }],
		['a copy naming an item twice', { type: 'submit', direction: 'upload', srcRoot: '0', srcPath: 'Users', names: ['a', 'a'], dstRoot: '0', dstPath: 'x' }],
		['a copy with a path for a name', { type: 'submit', direction: 'upload', srcRoot: '0', srcPath: 'Users', names: ['a/b'], dstRoot: '0', dstPath: 'x' }],
		['a copy with no names', { type: 'submit', direction: 'upload', srcRoot: '0', srcPath: 'Users/someone/a.txt', dstRoot: '0', dstPath: 'x' }],
		['an overwrite applied to the rest', { type: 'resolve', jobId: '3', choice: 'overwrite', applyToRemaining: true }],
		['a progress poll, since progress is pushed', { type: 'poll', jobId: '3' }],
		['a watch on a path outside its root', { type: 'watch', folder: { root: '2', path: '../etc' } }],
		['an adoption naming a job, since it takes over every interrupted one', { type: 'adopt', jobId: '3' }],
	];
	for (const [name, operation] of refused) {
		test(`${name} is refused`, () => {
			assert.strictEqual(isFileOperation(operation), false);
		});
	}
});

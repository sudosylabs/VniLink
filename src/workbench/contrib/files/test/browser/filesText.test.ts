// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import type { FileJob, FilePlaces } from '../../../../../platform/engine/common/files.js';
import { folderName, formatSize, hiddenNames, itemCount, jobStatus } from '../../filesText.js';

/** A transfer of `a.txt` under way, 40 of its 100 bytes sent. */
const running: FileJob = {
	id: '1', peer: 'p', label: 'a.txt', direction: 'Upload', sender: true, state: 'Transferring', total_bytes: '100', done_bytes: '40', files_total: '1', files_done: '0',
	resumed_files: '0', skipped_files: '0', skipped_symlinks: '0', waiting_file: null, current_file: null, error: null, destination: null,
};

suite('Files text', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const sizes: readonly (readonly [number, string])[] = [
		[4, '4 bytes'],
		[4000, '4 KB'],
		[38_200_000, '38.2 MB'],
		[412_000_000, '412 MB'],
		[1_100_000_000, '1.1 GB'],
	];
	for (const [bytes, text] of sizes) {
		test(`${String(bytes)} bytes read as ${text}`, () => {
			assert.strictEqual(formatSize(bytes), text);
		});
	}
	const places: FilePlaces = { roots: [{ id: '0', name: '/' }, { id: '2', name: 'C:' }], folders: [] };
	test('one name that is not text is said in the singular', () => {
		assert.strictEqual(hiddenNames(1), '1 name could not be shown');
	});
	test('several names that are not text are counted', () => {
		assert.strictEqual(hiddenNames(3), '3 names could not be shown');
	});
	test('a folder is named by its last part', () => {
		assert.strictEqual(folderName({ root: '0', path: 'Users/someone/Downloads' }, places, 'This Mac'), 'Downloads');
	});
	test('the whole disk takes its computer\'s name', () => {
		assert.strictEqual(folderName({ root: '0', path: '' }, places, 'This Mac'), 'This Mac');
	});
	test('a drive root other than the disk\'s is named by its drive', () => {
		assert.strictEqual(folderName({ root: '2', path: '' }, places, 'Studio PC'), 'C:');
	});
	test('one item is counted in the singular', () => {
		assert.strictEqual(itemCount(1), '1 item');
	});
	test('several items are counted in the plural', () => {
		assert.strictEqual(itemCount(3), '3 items');
	});
	const statuses: readonly (readonly [string, FileJob, number | undefined, string])[] = [
		['a transfer without a rate', running, undefined, '40 bytes of 100 bytes'],
		['a transfer with a rate', { ...running, done_bytes: '500000000', total_bytes: '1500000000' }, 20_000_000, '500 MB of 1.5 GB · 20 MB/s · 50 s left'],
		['a transfer waiting on a name already there', { ...running, state: 'WaitingDecision', waiting_file: 'Reports/q3.xlsx' }, undefined, '“q3.xlsx” is already there'],
		['an interrupted transfer', { ...running, state: 'Interrupted' }, undefined, 'Stopped at 40 bytes of 100 bytes'],
		['a transfer interrupted before a byte moved', { ...running, state: 'Interrupted', done_bytes: '0' }, undefined, 'Stopped: the connection was lost'],
		['an interrupted transfer with a reason', { ...running, state: 'Interrupted', error: 'the transfer stalled' }, undefined, 'Stopped at 40 bytes of 100 bytes: the transfer stalled'],
		['a transfer interrupted with a reason before a byte moved', { ...running, state: 'Interrupted', done_bytes: '0', error: 'file transfer is no longer allowed' }, undefined, 'Stopped: file transfer is no longer allowed'],
		['a running transfer with a reason', { ...running, error: 'the other computer could not be reached' }, undefined, 'Trying again: the other computer could not be reached'],
		['a transfer of several files', { ...running, files_total: '3', current_file: 'Photos/b.jpg' }, undefined, 'b.jpg · 40 bytes of 100 bytes'],
		['a transfer of one file, which needs no name', { ...running, current_file: 'a.txt' }, undefined, '40 bytes of 100 bytes'],
		['a failed transfer', { ...running, state: 'Failed', error: 'there is not enough space' }, undefined, 'there is not enough space'],
		['a complete transfer', { ...running, state: 'Complete', files_total: '86', total_bytes: '212000000' }, undefined, '86 files, 212 MB, checked'],
	];
	for (const [name, job, rate, text] of statuses) {
		test(`${name} reads "${text}"`, () => {
			assert.strictEqual(jobStatus(job, rate), text);
		});
	}
});

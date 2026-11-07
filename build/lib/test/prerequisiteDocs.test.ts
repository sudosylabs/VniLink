// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { contributingWithPrerequisites } from '../prerequisitesDocs.ts';

const { current, updated } = await contributingWithPrerequisites(fileURLToPath(new URL('../../../', import.meta.url)));

const latencyRows = current.split('\n').filter(line => line.startsWith('| `npm run measure:latency` |'));

suite('prerequisiteDocs', () => {
	test('CONTRIBUTING.md carries the setup section generated from the manifest', () => {
		assert.strictEqual(current, updated, 'Run `node build/run.ts docs:prerequisites`');
	});
	test('links nothing under docs/, which is not published', () => {
		assert.doesNotMatch(current, /\]\(docs\//);
	});
	test('lists the latency harness', () => {
		assert.notStrictEqual(latencyRows.length, 0);
	});
	for (const [index, row] of latencyRows.entries()) {
		test(`lists a second computer as optional for the latency harness (row ${String(index + 1)})`, () => {
			assert.match(row, /A second computer \(optional\)/);
		});
		test(`reaches no other computer over OpenSSH for the latency harness (row ${String(index + 1)})`, () => {
			assert.doesNotMatch(row, /OpenSSH/);
		});
	}
	test('joined steps do not end their own sentences', () => {
		assert.doesNotMatch(current, /\.; /);
	});
});

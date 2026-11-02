// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { suite, test } from 'node:test';
import { checkoutVolume, linuxValidationVolumes } from '../containers.ts';

suite('containers', () => {
	const main = '/work/vnilink';
	const worktree = '/work/vnilink/.claude/worktrees/one';

	/** The volume each mount target of a checkout's Linux validation uses. */
	function volumes(root: string): Map<string, string> {
		const sources = new Map<string, string>();
		for (const mount of linuxValidationVolumes(root).filter(argument => argument !== '--mount')) {
			const match = /source=([^,]+),target=([^,]+)$/.exec(mount);
			assert.ok(match, `not a volume mount: ${mount}`);
			sources.set(match[2], match[1]);
		}
		return sources;
	}
	test('names a checkout volume after its cache and a digest of the checkout', () => {
		assert.match(checkoutVolume('vnilink-impaired-release-target', main), /^vnilink-impaired-release-target-[0-9a-f]{12}$/);
	});
	test('gives each checkout a volume of its own', () => {
		assert.notStrictEqual(checkoutVolume('vnilink-impaired-release-target', main), checkoutVolume('vnilink-impaired-release-target', worktree));
	});
	test('gives a checkout the same volume every time', () => {
		assert.strictEqual(checkoutVolume('vnilink-impaired-release-target', main), checkoutVolume('vnilink-impaired-release-target', main));
	});
	test('mounts the Cargo registry, the target and node_modules for Linux validation', () => {
		assert.deepStrictEqual([...volumes(main).keys()], ['/usr/local/cargo/registry', '/work/target', '/work/node_modules']);
	});
	test('shares the Cargo registry between checkouts', () => {
		assert.strictEqual(volumes(main).get('/usr/local/cargo/registry'), volumes(worktree).get('/usr/local/cargo/registry'));
	});
	// Doctests link against the target's libraries; another checkout's build must not replace them mid-run.
	for (const target of ['/work/target', '/work/node_modules']) {
		test(`gives each checkout its own ${target}`, () => {
			assert.notStrictEqual(volumes(main).get(target), volumes(worktree).get(target));
		});
	}
});

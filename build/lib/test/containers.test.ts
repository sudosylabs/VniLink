// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { readFile } from 'node:fs/promises';
import { suite, test } from 'node:test';
import { checkoutVolume, containerVolumes, imageToolchain } from '../containers.ts';

const dockerfile = await readFile(new URL('../../containers/Dockerfile.linux', import.meta.url), 'utf8');

suite('containers', () => {
	const main = '/work/vnilink';
	const worktree = '/work/vnilink/.claude/worktrees/one';

	/** The volume each mount target of a checkout's Linux validation uses. */
	function volumes(root: string, toolchain = 'trixie-rust191'): Map<string, string> {
		const sources = new Map<string, string>();
		for (const mount of containerVolumes('linux', toolchain, root).filter(argument => argument !== '--mount')) {
			const match = /source=([^,]+),target=([^,]+)$/.exec(mount);
			assert.ok(match, `not a volume mount: ${mount}`);
			sources.set(match[2], match[1]);
		}
		return sources;
	}
	test('reads the image\'s distribution and Rust release from its Dockerfile', () => {
		assert.match(imageToolchain(dockerfile), /^[a-z]+-rust\d+$/);
		assert.strictEqual(imageToolchain('FROM node:22 AS node\nFROM rust:1.91.0-trixie@sha256:00\n'), 'trixie-rust191');
	});
	test('gives another toolchain another target, and keeps node_modules', () => {
		assert.notStrictEqual(volumes(main).get('/work/target'), volumes(main, 'trixie-rust192').get('/work/target'));
		assert.strictEqual(volumes(main).get('/work/node_modules'), volumes(main, 'trixie-rust192').get('/work/node_modules'));
	});
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

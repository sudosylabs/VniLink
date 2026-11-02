// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { readFileSync } from 'node:fs';
import { join, posix } from 'node:path';
import { suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../../', import.meta.url));

/** The `[patch.crates-io]` entries of a manifest, as crate name to the path relative to the repository root. */
function patches(manifest: string, directory: string): Map<string, string> {
	const section = /^\[patch\.crates-io\]\n([\s\S]*?)(?=^\[|$(?![\s\S]))/m.exec(readFileSync(join(root, manifest), 'utf8'))?.[1] ?? '';
	return new Map([...section.matchAll(/^([\w-]+) = \{ path = "([^"]+)" \}/gm)].map(([, name, path]) => [name, posix.normalize(posix.join(directory, path))]));
}

/** The packages a lockfile resolves. */
function locked(lockfile: string): Set<string> {
	return new Set([...readFileSync(join(root, lockfile), 'utf8').matchAll(/^name = "([^"]+)"/gm)].map(([, name]) => name));
}

suite('the fuzz crate', () => {
	// The fuzz crate is outside the workspace, so it repeats the workspace's patches; a missing one builds the
	// published crate instead of the vendored one, and every fuzz target then fails to build.
	const workspace = patches('Cargo.toml', '.');
	const fuzz = patches('crates/vnilink/fuzz/Cargo.toml', 'crates/vnilink/fuzz');
	const built = locked('crates/vnilink/fuzz/Cargo.lock');
	for (const [name, path] of workspace) {
		if (built.has(name)) {
			test(`builds ${name} from the vendored source the workspace builds`, () => {
				assert.strictEqual(fuzz.get(name), path);
			});
		}
	}
	test('patches only crates the workspace patches, from the same sources', () => {
		for (const [name, path] of fuzz) {
			assert.strictEqual(workspace.get(name), path, `${name} is patched by the fuzz crate alone`);
		}
	});
});

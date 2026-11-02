// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { readFile } from 'node:fs/promises';
import { join, posix } from 'node:path';
import { suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../../', import.meta.url));

/** The `[patch.crates-io]` entries of a manifest, as crate name to the path relative to the repository root. */
async function patches(manifest: string, directory: string): Promise<Map<string, string>> {
	const section = /^\[patch\.crates-io\]\n([\s\S]*?)(?=^\[|$(?![\s\S]))/m.exec(await readFile(join(root, manifest), 'utf8'))?.[1] ?? '';
	return new Map([...section.matchAll(/^([\w-]+) = \{ path = "([^"]+)" \}/gm)].map(([, name, path]) => [name, posix.normalize(posix.join(directory, path))]));
}

/** The packages a lockfile resolves. */
async function locked(lockfile: string): Promise<Set<string>> {
	return new Set([...(await readFile(join(root, lockfile), 'utf8')).matchAll(/^name = "([^"]+)"/gm)].map(([, name]) => name));
}

// The fuzz crate is outside the workspace, so it repeats the workspace's patches; a missing one builds the
// published crate instead of the vendored one, and every fuzz target then fails to build.
const workspace = await patches('Cargo.toml', '.');

const fuzz = await patches('crates/vnilink/fuzz/Cargo.toml', 'crates/vnilink/fuzz');

const built = await locked('crates/vnilink/fuzz/Cargo.lock');

suite('the fuzz crate', () => {
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

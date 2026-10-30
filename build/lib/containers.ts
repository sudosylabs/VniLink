// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Docker volumes shared by the Linux validation and impaired-network containers. */
import { createHash } from 'node:crypto';

/**
 * A cache volume for one checkout: worktrees of the same repository build
 * different code, and a shared target would let one run link or execute
 * another's artifacts.
 */
export function checkoutVolume(name: string, root: string): string {
	return `${name}-${createHash('sha256').update(root).digest('hex').slice(0, 12)}`;
}

/**
 * The writable caches of Linux validation. The Cargo registry is shared: its
 * downloads are content-addressed and Cargo locks them. The target and
 * `node_modules` hold this checkout's build, so each checkout has its own.
 */
export function linuxValidationVolumes(root: string): string[] {
	return [
		'--mount', 'type=volume,source=vnilink-linux-cargo-registry,target=/usr/local/cargo/registry',
		'--mount', `type=volume,source=${checkoutVolume('vnilink-linux-trixie-rust191-target', root)},target=/work/target`,
		'--mount', `type=volume,source=${checkoutVolume('vnilink-linux-node-modules', root)},target=/work/node_modules`,
	];
}

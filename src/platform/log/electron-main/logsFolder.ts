// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { readdir, rm } from 'node:fs/promises';
import { basename, join } from 'node:path';

/** How many runs' logs folders are kept, this run's included. */
const KEPT_RUNS = 10;

/**
 * A run's logs folder name from when it started, as VS Code names
 * `logsHome`: the ISO time without separators or milliseconds, so names
 * sort in time order.
 */
export function logsFolderName(startedAt: Date): string {
	return startedAt.toISOString().replace(/-|:|\.\d+Z$/g, '');
}

/**
 * Removes all but the newest {@link KEPT_RUNS} runs' folders under `root`,
 * never `current`; what VS Code's logs data cleaner does in the workbench.
 */
export async function pruneLogs(root: string, current: string, kept = KEPT_RUNS): Promise<void> {
	const runs = (await readdir(root, { withFileTypes: true }).catch(() => []))
		.filter(entry => entry.isDirectory() && /^\d{8}T\d{6}$/.test(entry.name) && entry.name !== basename(current))
		.map(entry => entry.name)
		.sort();
	const removed = runs.slice(0, Math.max(0, runs.length - (kept - 1)));
	await Promise.all(removed.map(name => rm(join(root, name), { recursive: true, force: true })));
}

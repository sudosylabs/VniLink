// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { processOutput } from './processes.ts';

/** Third-party sources, which keep their own notices and conventions. */
export const thirdPartyDirectories: readonly string[] = ['vendor/'];

/**
 * Git's output for `args` in `root`. A git hook's GIT_DIR and GIT_INDEX_FILE
 * name the repository committing, not necessarily `root`'s, so they are dropped.
 */
export async function git(root: string, args: readonly string[]): Promise<string> {
	const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith('GIT_')));
	return processOutput('git', ['-C', root, ...args], { env });
}

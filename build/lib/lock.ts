// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

/** Whether a process with `pid` still runs on this computer. */
function running(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch (error) {
		// EPERM: it runs as someone else.
		return (error as NodeJS.ErrnoException).code === 'EPERM';
	}
}

/**
 * Takes the lock directory `path` for this process, so two builds never
 * publish over each other. A lock whose process is gone, as after a crash
 * or a killed build, is taken over; one held by a running build is refused,
 * naming it. The returned function releases the lock.
 */
export async function acquireLock(path: string): Promise<() => Promise<void>> {
	for (let attempt = 0; attempt < 2; attempt++) {
		try {
			await mkdir(path);
			await writeFile(join(path, 'owner.json'), `${JSON.stringify({ pid: process.pid })}\n`);
			return () => rm(path, { recursive: true, force: true });
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== 'EEXIST') {
				throw error;
			}
			const owner = await readFile(join(path, 'owner.json'), 'utf8').then(text => (JSON.parse(text) as { pid?: unknown }).pid, () => undefined);
			if (typeof owner === 'number' && running(owner)) {
				throw new Error(`Another build (process ${String(owner)}) is publishing here; wait for it, or stop it and run again (${path})`);
			}
			// Its owner is gone, or it is too old to name one: a build that stopped without releasing it.
			await rm(path, { recursive: true, force: true });
		}
	}
	throw new Error(`The lock ${path} could not be taken`);
}

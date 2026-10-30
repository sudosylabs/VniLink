// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Makes a development build its own app for macOS privacy permissions. */
import { execFile } from 'node:child_process';
import { mkdir, stat } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { promisify } from 'node:util';

const run = promisify(execFile);

/** A program and its arguments, as `ProcessRunner.start` takes them. */
export interface Launch {
	readonly command: string;
	readonly args: readonly string[];
}

async function modified(path: string): Promise<number | undefined> {
	try {
		return (await stat(path)).mtimeMs;
	} catch {
		return undefined;
	}
}

/**
 * How to start `executable` so it owns its privacy permissions. macOS gives a
 * process the camera, microphone, screen recording and accessibility access of
 * the app that launched it, so a build started by a terminal, an editor or an
 * agent would ask for theirs; `build/lib/macos/disclaim.c` starts it
 * responsible for itself instead. Elsewhere the executable starts as it is.
 */
export async function selfResponsible(
	root: string,
	executable: string,
	args: readonly string[],
	signal: AbortSignal,
	platform = process.platform,
): Promise<Launch> {
	if (platform !== 'darwin') {
		return { command: executable, args };
	}
	const source = join(root, 'build/lib/macos/disclaim.c');
	const launcher = join(root, '.build/dev/disclaim');
	const built = await modified(launcher);
	const changed = await modified(source);
	if (built === undefined || changed === undefined || changed > built) {
		await mkdir(dirname(launcher), { recursive: true });
		await run('xcrun', ['clang', '-O2', '-Wall', '-Werror', '-o', launcher, source], { signal });
	}
	return { command: launcher, args: [executable, ...args] };
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The spdlog native logger, loaded by path from `native/spdlog` beside the
 * bundled entry point, where the build stages it: the packaged app ships no
 * node_modules. It stands in for the `@vscode/spdlog` package's own entry,
 * whose helpers find and wrap the same addon, and offers what
 * `spdlogLog.ts` uses of it. The package is optional: where it did not
 * build, loading fails and that logger writes nothing, as upstream does.
 */
import { mkdir } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';

/** One spdlog logger; see `@vscode/spdlog`'s `Logger`. */
export interface Logger {
	trace(message: string): void;
	debug(message: string): void;
	info(message: string): void;
	warn(message: string): void;
	error(message: string): void;
	critical(message: string): void;
	getLevel(): number;
	setLevel(level: number): void;
	setPattern(pattern: string): void;
	clearFormatters(): void;
	flush(): void;
	drop(): void;
}

interface Addon {
	setFlushOn(level: number): void;
	Logger: new (type: 'rotating' | 'rotating_async', name: string, filename: string, filesize: number, filecount: number) => Logger;
}

/** Where the build stages the addon, relative to the bundled entry point. */
export const SPDLOG_DIRECTORY = join('native', 'spdlog');

/** The package entry's two functions spdlogLog.ts calls. */
export interface Spdlog {
	setFlushOn(level: number): void;
	createAsyncRotatingLogger(name: string, filename: string, filesize: number, filecount: number): Promise<Logger>;
}

/** The staged addon, as {@link Spdlog}. */
export function loadSpdlog(): Spdlog {
	const addon = createRequire(__filename)(join(__dirname, SPDLOG_DIRECTORY, 'spdlog.node')) as Addon;
	return {
		setFlushOn: level => {
			addon.setFlushOn(level);
		},
		createAsyncRotatingLogger: async (name, filename, filesize, filecount) => {
			await mkdir(dirname(filename), { recursive: true });
			return new addon.Logger('rotating_async', name, filename, filesize, filecount);
		},
	};
}

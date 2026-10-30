/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/platform/terminal/common/terminal.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import type { Event } from '../../../base/common/event.js';
import type { IDisposable } from '../../../base/common/lifecycle.js';

/** The widest grid the host accepts (`TerminalSize` in crates/vnilink/src/terminal.rs). */
export const MAX_TERMINAL_COLUMNS = 1024;

/** The tallest grid the host accepts. */
export const MAX_TERMINAL_ROWS = 512;

/**
 * Output for the terminal. The host sends nothing more until `acknowledge`
 * runs, so it is called once the emulator has parsed `data`, as VS Code
 * acknowledges from xterm's write callback (`TerminalInstance._writeProcessData`).
 */
export interface IProcessDataEvent {
	readonly data: string | Uint8Array;
	acknowledge(): void;
}

/** Why a shell could not start, in the terms its computer shares. */
export type TerminalLaunchFailure = 'disconnected' | 'notAllowed' | 'unsupported' | 'unavailable' | 'failed';

export interface ITerminalLaunchError {
	readonly code: TerminalLaunchFailure;
}

/** How a running shell ended: its exit code, a failure on its computer, or the connection. */
export type TerminalExit = { readonly code: number } | { readonly reason: 'failed' | 'lost' };

/**
 * The process behind a terminal view. This is the part of VS Code's
 * `ITerminalChildProcess` (platform/terminal/common/terminal.ts) a shell on
 * another computer has: its host chooses the program, directory and
 * environment, so properties, cwd, signals and replay are not offered.
 */
export interface ITerminalChildProcess extends IDisposable {
	readonly onProcessData: Event<IProcessDataEvent>;
	readonly onProcessReady: Event<void>;
	readonly onProcessExit: Event<TerminalExit>;

	/** Starts the shell; resolves to the reason when it could not start. */
	start(): Promise<ITerminalLaunchError | undefined>;
	input(data: string): void;
	/** Sends bytes given one per character, as xterm reports some mouse modes. */
	processBinary(data: string): void;
	resize(cols: number, rows: number): void;
	/** Ends the shell. No events follow. */
	shutdown(): void;
}

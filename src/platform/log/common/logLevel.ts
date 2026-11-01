/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/platform/log/common/log.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

/**
 * The log levels, apart from the rest of `log.ts` so that the preload can
 * check a window's configuration without loading the logging services.
 */
export enum LogLevel {
	Off,
	Trace,
	Debug,
	Info,
	Warning,
	Error
}

const levels: ReadonlySet<number> = new Set([LogLevel.Off, LogLevel.Trace, LogLevel.Debug, LogLevel.Info, LogLevel.Warning, LogLevel.Error]);

/** Whether `value` is one of the levels, not merely a number. */
export function isKnownLogLevel(value: unknown): value is LogLevel {
	return typeof value === 'number' && levels.has(value);
}

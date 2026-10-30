/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

// Declare types that we probe for to implement util and/or polyfill functions

declare global {

	// --- idle callbacks

	interface IdleDeadline {
		readonly didTimeout: boolean;
		timeRemaining(): number;
	}

	function requestIdleCallback(callback: (args: IdleDeadline) => void, options?: { timeout: number }): number;
	function cancelIdleCallback(handle: number): void;


	// --- timeout / interval (available in all contexts, but different signatures in node.js vs web)

	type Timeout = ReturnType<typeof globalThis.setTimeout>;

	// --- error

	interface ErrorConstructor {
		captureStackTrace(targetObject: object, constructorOpt?: (...args: never[]) => unknown): void;
		stackTraceLimit: number;
	}
}

export { };

/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: the sandboxed process comes from the `vnilink` global; the working directory is the process's own.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { INodeProcess, isMacintosh, isWindows } from './platform.js';

let safeProcess: Omit<INodeProcess, 'arch'> & { arch: string | undefined };
declare const process: INodeProcess;

// Native sandbox environment
const vnilinkGlobal = (globalThis as { vnilink?: { process?: INodeProcess } }).vnilink;
if (typeof vnilinkGlobal !== 'undefined' && typeof vnilinkGlobal.process !== 'undefined') {
	const sandboxProcess: INodeProcess = vnilinkGlobal.process;
	safeProcess = {
		get platform() { return sandboxProcess.platform; },
		get arch() { return sandboxProcess.arch; },
		get env() { return sandboxProcess.env; },
		cwd() { return sandboxProcess.cwd(); }
	};
}

// Native node.js environment
else if (typeof process !== 'undefined' && typeof process?.versions?.node === 'string') {
	safeProcess = {
		get platform() { return process.platform; },
		get arch() { return process.arch; },
		get env() { return process.env; },
		cwd() { return process.cwd(); }
	};
}

// Web environment
else {
	safeProcess = {

		// Supported
		get platform() { return isWindows ? 'win32' : isMacintosh ? 'darwin' : 'linux'; },
		get arch() { return undefined; /* arch is undefined in web */ },

		// Unsupported
		get env() { return {}; },
		cwd() { return '/'; }
	};
}

/**
 * Provides safe access to the `cwd` property in node.js, sandboxed or web
 * environments.
 *
 * Note: in web, this property is hardcoded to be `/`.
 *
 * @skipMangle
 */
export const cwd = safeProcess.cwd;

/**
 * Provides safe access to the `env` property in node.js, sandboxed or web
 * environments.
 *
 * Note: in web, this property is hardcoded to be `{}`.
 */
export const env = safeProcess.env;

/**
 * Provides safe access to the `platform` property in node.js, sandboxed or web
 * environments.
 */
export const platform = safeProcess.platform;

/**
 * Provides safe access to the `arch` method in node.js, sandboxed or web
 * environments.
 * Note: `arch` is `undefined` in web
 */
export const arch = safeProcess.arch;

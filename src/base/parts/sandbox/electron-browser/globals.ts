/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: the preload exposes its API as the `vnilink` global.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { INodeProcess, IProcessEnvironment } from '../../../common/platform.js';
import { ISandboxConfiguration } from '../common/sandboxTypes.js';
import { IpcRenderer, WebFrame, WebUtils } from './electronTypes.js';

/**
 * In Electron renderers we cannot expose all of the `process` global of node.js
 */
export interface ISandboxNodeProcess extends INodeProcess {

	/**
	 * The process.platform property returns a string identifying the operating system platform
	 * on which the Node.js process is running.
	 */
	readonly platform: string;

	/**
	 * The process.arch property returns a string identifying the CPU architecture
	 * on which the Node.js process is running.
	 */
	readonly arch: string;

	/**
	 * The type will always be `renderer`.
	 */
	readonly type: string;

	/**
	 * A list of versions for the current node.js/electron configuration.
	 */
	readonly versions: { [key: string]: string | undefined };

	/**
	 * The process.env property returns an object containing the user environment.
	 */
	readonly env: IProcessEnvironment;

	/**
	 * The current working directory of the process.
	 */
	cwd: () => string;
}

export interface IpcMessagePort {

	/**
	 * Acquire a `MessagePort`. The main process will transfer the port over to
	 * the `responseChannel` with a payload of `requestNonce` so that the source can
	 * correlate the response.
	 *
	 * The source should install a `window.on('message')` listener, ensuring `e.data`
	 * matches `nonce`, `e.source` matches `window` and then receiving the `MessagePort`
	 * via `e.ports[0]`.
	 */
	acquire(responseChannel: string, nonce: string): void;
}

export interface ISandboxContext {

	/**
	 * Allows to await the resolution of the configuration object.
	 */
	resolveConfiguration(): Promise<ISandboxConfiguration>;
}

interface ISandboxGlobal {
	vnilink: {
		readonly ipcRenderer: IpcRenderer;
		readonly ipcMessagePort: IpcMessagePort;
		readonly webFrame: WebFrame;
		readonly process: ISandboxNodeProcess;
		readonly context: ISandboxContext;
		readonly webUtils: WebUtils;
	};
}

const vnilinkGlobal = (globalThis as unknown as ISandboxGlobal).vnilink;
export const ipcRenderer: IpcRenderer = vnilinkGlobal.ipcRenderer;
export const ipcMessagePort: IpcMessagePort = vnilinkGlobal.ipcMessagePort;
export const context: ISandboxContext = vnilinkGlobal.context;
export const webUtils: WebUtils = vnilinkGlobal.webUtils;

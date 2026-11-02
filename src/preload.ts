/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/base/parts/sandbox/electron-browser/preload.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

/**
 * Sandboxed preload. It exposes the `vnilink` global that the copied IPC and
 * platform modules read (`createPreloadGlobals`), after asking main for the
 * window configuration.
 */
import { contextBridge, ipcRenderer, webUtils } from 'electron';
import { createPreloadGlobals } from './base/parts/sandbox/electron-browser/preloadGlobals.js';
import { isNativeWindowConfiguration, WINDOW_CONFIGURATION_CHANNEL, type INativeWindowConfiguration } from './platform/window/common/windowConfiguration.js';

if (!process.sandboxed || !process.contextIsolated) {
	throw new Error('Renderer isolation required');
}

let configuration: INativeWindowConfiguration | undefined;

const resolveConfiguration: Promise<INativeWindowConfiguration> = (async () => {
	const value: unknown = await ipcRenderer.invoke(WINDOW_CONFIGURATION_CHANNEL);
	if (!isNativeWindowConfiguration(value)) {
		throw new Error('Preload: invalid window configuration');
	}
	configuration = value;
	return value;
})();

const globals = createPreloadGlobals({
	ipcRenderer,
	getPathForFile: file => webUtils.getPathForFile(file),
	postPorts: (nonce, ports) => {
		window.postMessage(nonce, location.origin, [...ports]);
	},
}, {
	configuration: () => configuration,
	resolveConfiguration: () => resolveConfiguration,
});

contextBridge.exposeInMainWorld('vnilink', globals);

/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/base/parts/sandbox/electron-browser/preload.ts.
 *--------------------------------------------------------------------------------------------*/

/**
 * Sandboxed preload, shaped like VS Code's `base/parts/sandbox/electron-browser/preload.ts`.
 * It exposes the same `vscode` globals that the copied IPC and platform modules
 * read, with VniLink's stricter `MessagePort` handling and no environment.
 */
import { contextBridge, ipcRenderer, webUtils } from 'electron';
import type { IpcRendererEvent } from 'electron';
import { isNativeWindowConfiguration, WINDOW_CONFIGURATION_CHANNEL, type INativeWindowConfiguration } from './platform/window/common/windowConfiguration.js';

if (!process.sandboxed || !process.contextIsolated) {
	throw new Error('Renderer isolation required');
}

/** At most this many `MessagePort` acquisitions may wait at once. */
const maxPendingPorts = 8;

/** An acquisition that main never answers is forgotten after this delay. */
const portTimeout = 20_000;

function validateIPC(channel: string): true | never {
	if (!channel.startsWith('vscode:')) {
		throw new Error(`Unsupported event IPC channel '${channel}'`);
	}
	return true;
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

/** Pending acquisitions by response channel, then by nonce. */
const pendingPorts = new Map<string, Map<string, ReturnType<typeof setTimeout>>>();

function pendingPortCount(): number {
	let count = 0;
	for (const nonces of pendingPorts.values()) {
		count += nonces.size;
	}
	return count;
}

function onPortResponse(responseChannel: string) {
	return (event: IpcRendererEvent, nonce: unknown): void => {
		const nonces = pendingPorts.get(responseChannel);
		const timer = typeof nonce === 'string' ? nonces?.get(nonce) : undefined;
		if (!nonces || timer === undefined || typeof nonce !== 'string') {
			// Never forward a port this window did not ask for.
			for (const port of event.ports) {
				port.close();
			}
			return;
		}
		clearTimeout(timer);
		nonces.delete(nonce);
		window.postMessage(nonce, location.origin, event.ports);
	};
}

const globals = {

	ipcRenderer: {

		send(channel: string, ...args: unknown[]): void {
			validateIPC(channel);
			ipcRenderer.send(channel, ...args);
		},

		invoke(channel: string, ...args: unknown[]): Promise<unknown> {
			validateIPC(channel);
			return ipcRenderer.invoke(channel, ...args);
		},

		on(channel: string, listener: (event: IpcRendererEvent, ...args: unknown[]) => void) {
			validateIPC(channel);
			ipcRenderer.on(channel, listener);
			return this;
		},

		once(channel: string, listener: (event: IpcRendererEvent, ...args: unknown[]) => void) {
			validateIPC(channel);
			ipcRenderer.once(channel, listener);
			return this;
		},

		removeListener(channel: string, listener: (event: IpcRendererEvent, ...args: unknown[]) => void) {
			validateIPC(channel);
			ipcRenderer.removeListener(channel, listener);
			return this;
		},
	},

	ipcMessagePort: {

		/**
		 * Prepares to receive a `MessagePort` that main sends on `responseChannel`
		 * with `nonce`; the port is posted to this window with `nonce` as data.
		 */
		acquire(responseChannel: string, nonce: string): void {
			validateIPC(responseChannel);
			if (!/^[a-zA-Z0-9-]{1,64}$/.test(nonce) || pendingPortCount() >= maxPendingPorts) {
				throw new Error('MessagePort acquisition refused');
			}
			let nonces = pendingPorts.get(responseChannel);
			if (!nonces) {
				nonces = new Map();
				pendingPorts.set(responseChannel, nonces);
				ipcRenderer.on(responseChannel, onPortResponse(responseChannel));
			}
			if (nonces.has(nonce)) {
				throw new Error('MessagePort acquisition refused');
			}
			const pending = nonces;
			pending.set(nonce, setTimeout(() => {
				pending.delete(nonce);
			}, portTimeout));
		},
	},

	/** The subset of `process` the copied platform modules read; the environment stays private. */
	process: {
		get platform() {
			return process.platform;
		},
		get arch() {
			return process.arch;
		},
		get env() {
			return {};
		},
		get versions() {
			return process.versions;
		},
		get type() {
			return 'renderer';
		},
		cwd(): string {
			return '/';
		},
	},

	/** As VS Code's: the path of a file dropped from the system's file manager. */
	webUtils: {
		getPathForFile(file: File): string {
			return webUtils.getPathForFile(file);
		},
	},

	context: {
		configuration(): INativeWindowConfiguration | undefined {
			return configuration;
		},
		resolveConfiguration(): Promise<INativeWindowConfiguration> {
			return resolveConfiguration;
		},
	},
};

contextBridge.exposeInMainWorld('vscode', globals);

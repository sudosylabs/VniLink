/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/base/parts/sandbox/electron-browser/preload.ts: the globals the
 *  preload exposes, built from the Electron pieces it is given, with stricter `MessagePort` handling and no environment.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import type { IpcRendererEvent } from 'electron';

/** At most this many `MessagePort` acquisitions may wait at once. */
const maxPendingPorts = 8;

/** An acquisition that main never answers is forgotten after this delay. */
const portTimeout = 20_000;

type RendererListener = (event: IpcRendererEvent, ...args: unknown[]) => void;

/** The part of Electron's `ipcRenderer` the globals use. */
export interface IPreloadIpcRenderer {
	send(channel: string, ...args: unknown[]): void;
	on(channel: string, listener: RendererListener): unknown;
	once(channel: string, listener: RendererListener): unknown;
	removeListener(channel: string, listener: RendererListener): unknown;
}

/** What the globals need from Electron and the page; the preload passes the real ones. */
interface IPreloadHost {
	readonly ipcRenderer: IPreloadIpcRenderer;
	getPathForFile(file: File): string;
	/** Hands the page the ports main sent for `nonce`, with `nonce` as the message data. */
	postPorts(nonce: string, ports: readonly MessagePort[]): void;
}

/** The window configuration as the page reads it, before and after main answered. */
interface IPreloadContext<C> {
	configuration(): C | undefined;
	resolveConfiguration(): Promise<C>;
}

type PageListener = (event: object, ...args: unknown[]) => void;

function validateIPC(channel: string): true | never {
	if (!channel.startsWith('vnilink:')) {
		throw new Error(`Unsupported event IPC channel '${channel}'`);
	}
	return true;
}

/**
 * The `vnilink` global the copied IPC and platform modules read. The page
 * sends and listens only on `vnilink:` channels, invokes nothing, receives
 * only the ports it asked for, and sees no environment.
 */
export function createPreloadGlobals<C>(host: IPreloadHost, context: IPreloadContext<C>) {
	const { ipcRenderer } = host;

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
			host.postPorts(nonce, event.ports);
		};
	}

	/** The bridge's listener for each page listener, so `removeListener` finds it. */
	const forwarders = new WeakMap<PageListener, (event: IpcRendererEvent, ...args: unknown[]) => void>();

	/**
	 * The page gets a message's arguments and an empty event: Electron's event
	 * carries `sender`, the unchecked ipcRenderer, and `ports`, which only the
	 * port relay below may hand over.
	 */
	function forwarder(listener: PageListener): (event: IpcRendererEvent, ...args: unknown[]) => void {
		let forward = forwarders.get(listener);
		if (!forward) {
			forward = (_event, ...args) => {
				listener({}, ...args);
			};
			forwarders.set(listener, forward);
		}
		return forward;
	}
	return {

		ipcRenderer: {

			send(channel: string, ...args: unknown[]): void {
				validateIPC(channel);
				ipcRenderer.send(channel, ...args);
			},

			on(channel: string, listener: PageListener) {
				validateIPC(channel);
				ipcRenderer.on(channel, forwarder(listener));
				return this;
			},

			once(channel: string, listener: PageListener) {
				validateIPC(channel);
				ipcRenderer.once(channel, forwarder(listener));
				return this;
			},

			removeListener(channel: string, listener: PageListener) {
				validateIPC(channel);
				ipcRenderer.removeListener(channel, forwarder(listener));
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

			/** Gives up an acquisition main will not answer, such as one whose request it refused. */
			release(responseChannel: string, nonce: string): void {
				validateIPC(responseChannel);
				const nonces = pendingPorts.get(responseChannel);
				clearTimeout(nonces?.get(nonce));
				nonces?.delete(nonce);
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

		/** The path of a file dropped from the system's file manager. */
		webUtils: {
			getPathForFile(file: File): string {
				return host.getPathForFile(file);
			},
		},

		context: {
			configuration(): C | undefined {
				return context.configuration();
			},
			resolveConfiguration(): Promise<C> {
				return context.resolveConfiguration();
			},
		},
	};
}

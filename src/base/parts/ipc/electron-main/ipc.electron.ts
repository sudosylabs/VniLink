/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: the connection context is assigned from the sender's window, never read from the renderer.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { BrowserWindow, WebContents } from 'electron';
import { validatedIpcMain } from './ipcMain.js';
import { VSBuffer } from '../../../common/buffer.js';
import { Emitter, Event } from '../../../common/event.js';
import { IDisposable, toDisposable } from '../../../common/lifecycle.js';
import { BufferWriter, ClientConnectionEvent, IPCServer, serialize } from '../common/ipc.js';
import { Protocol as ElectronProtocol } from '../common/ipc.electron.js';

interface IIPCEvent {
	event: { sender: WebContents };
	message: Buffer | null;
}

function createScopedOnMessageEvent(senderId: number, eventName: string): Event<VSBuffer | null> {
	const onMessage = Event.fromNodeEventEmitter<IIPCEvent>(validatedIpcMain, eventName, (event, message) => ({ event, message }));
	const onMessageFromSender = Event.filter(onMessage, ({ event }) => event.sender.id === senderId);

	return Event.map(onMessageFromSender, ({ message }) => message ? VSBuffer.wrap(message) : message);
}

/**
 * Replaces the client's first message, which `IPCServer` reads as the
 * connection context, with the context main derives from the sender.
 */
function withTrustedContext(onMessage: Event<VSBuffer>, context: string): Event<VSBuffer> {
	let first = true;
	return Event.map(onMessage, message => {
		if (!first) {
			return message;
		}
		first = false;
		const writer = new BufferWriter();
		serialize(writer, context);
		return writer.buffer;
	});
}

/**
 * An implementation of `IPCServer` on top of Electron `ipcMain` API.
 */
export class Server extends IPCServer {

	private static readonly Clients = new Map<number, IDisposable>();

	private static getOnDidClientConnect(): Event<ClientConnectionEvent> {
		const onHello = Event.fromNodeEventEmitter<WebContents>(validatedIpcMain, 'vscode:hello', ({ sender }) => sender);

		const onHelloFromWindow = Event.filter(onHello, webContents => BrowserWindow.fromWebContents(webContents) !== null);

		return Event.map(onHelloFromWindow, webContents => {
			const id = webContents.id;
			const windowId = BrowserWindow.fromWebContents(webContents)?.id;
			const client = Server.Clients.get(id);

			client?.dispose();

			const onDidClientReconnect = new Emitter<void>();
			const reconnectDisposable = toDisposable(() => {
				onDidClientReconnect.fire();
			});
			Server.Clients.set(id, reconnectDisposable);

			const onMessage = withTrustedContext(createScopedOnMessageEvent(id, 'vscode:message') as Event<VSBuffer>, `window:${windowId}`);
			const onDidClientDisconnect = Event.any(Event.signal(createScopedOnMessageEvent(id, 'vscode:disconnect')), onDidClientReconnect.event);
			Event.once(onDidClientDisconnect)(() => {
				if (Server.Clients.get(id) === reconnectDisposable) {
					Server.Clients.delete(id);
				}

				onDidClientReconnect.dispose();
			});
			const protocol = new ElectronProtocol(webContents, onMessage);

			return { protocol, onDidClientDisconnect };
		});
	}

	constructor() {
		super(Server.getOnDidClientConnect());
	}
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IpcMainInvokeEvent, WebContents } from 'electron';
import { EventEmitter } from 'events';
import type { IIpcMain, IValidatedIpcMainHost } from '../../electron-main/ipcMain.js';

type Handler = (event: IpcMainInvokeEvent, ...args: unknown[]) => unknown;

/**
 * Electron's `ipcMain`, with the application windows it knows: a test sends
 * messages with `emit` and invokes handlers with `invoke`.
 */
export class TestIpcMain extends EventEmitter implements IValidatedIpcMainHost {
	private readonly handlers = new Map<string, Handler>();
	/** The web contents of application windows. */
	public readonly windows = new Set<object>();

	public get ipcMain(): IIpcMain {
		return this;
	}

	public handle(channel: string, handler: Handler): void {
		this.handlers.set(channel, handler);
	}

	public removeHandler(channel: string): void {
		this.handlers.delete(channel);
	}

	public invoke(channel: string, event: IpcMainInvokeEvent): unknown {
		const handler = this.handlers.get(channel);
		if (!handler) {
			throw new Error(`No handler for ${channel}`);
		}
		return handler(event);
	}

	public isApplicationWindow(contents: WebContents): boolean {
		return this.windows.has(contents);
	}
}

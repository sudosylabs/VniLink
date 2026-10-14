/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import type { Event } from '../../../base/common/event.js';
import { Emitter } from '../../../base/common/event.js';
import type { IChannel } from '../../../base/parts/ipc/common/ipc.js';
import type { IMainProcessService } from '../common/mainProcessService.js';
import type { DesktopApi } from '../common/desktopApi.js';
import { isNativeHostRequest, isWindowState, type IWindowState } from '../../native/common/native.js';
export class MainProcessService extends Disposable implements IMainProcessService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<IWindowState>());
	private revision = 0;
	constructor(
		private readonly api: DesktopApi,
		private readonly windowId: number,
	) {
		super();
		this._register(toDisposable(api.onWindowStateChanged(state => {
			this.acceptState(state);
		})));
	}
	private acceptState(state: unknown): void {
		if (isWindowState(state) && state.windowId === this.windowId && state.revision > this.revision) {
			this.revision = state.revision;
			this.changed.fire(state);
		}
	}

	public getChannel(channelName: string): IChannel {
		if (channelName !== 'nativeHost') {
			throw new Error('Unknown main process channel');
		}
		return {
			call: async <T>(command: string, args: unknown): Promise<T> => {
				if (!isNativeHostRequest(command, args) || !Array.isArray(args) || args[0] !== this.windowId) {
					throw new Error('Invalid native host request');
				}
				const reply = await this.api.nativeHostCommand(command, args);
				if (command === 'getWindowState') {
					if (!isWindowState(reply) || reply.windowId !== this.windowId) {
						throw new Error('Invalid window state');
					}
					this.acceptState(reply);
				}
				return reply as T;
			},
			listen: <T>(event: string) => {
				if (event !== 'onDidChangeWindowState') {
					throw new Error('Unknown native host event');
				}
				return this.changed.event as Event<T>;
			},
		};
	}
}

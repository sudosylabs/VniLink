/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import type { BrowserWindow } from 'electron';
import { Disposable, DisposableStore } from '../../../base/common/lifecycle.js';
import { Emitter, Event } from '../../../base/common/event.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import type { IWindowState, IWindowControlsOptions } from '../common/native.js';
import { CodeWindow } from '../../windows/electron-main/windowImpl.js';
export const INativeHostMainService = createDecorator<NativeHostMainService>('nativeHostMainService');
export class NativeHostMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly stateChanged = this._register(new Emitter<IWindowState>());
	public readonly onDidChangeWindowState = this.stateChanged.event;
	private readonly revisions = new Map<number, number>();
	constructor(private readonly getWindow: () => BrowserWindow | null) {
		super();
	}
	public registerWindow(window: BrowserWindow): void {
		const store = this._register(new DisposableStore());
		const publish = (): void => {
			if (!window.isDestroyed()) {
				this.stateChanged.fire(this.state(window));
			}
		};
		const events = [
			'focus',
			'blur',
			'maximize',
			'unmaximize',
			'enter-full-screen',
			'leave-full-screen',
			'always-on-top-changed',
			'resize',
		] as const;
		for (const event of events) {
			store.add(Event.fromNodeEventEmitter<unknown>(window, event)(publish));
		}
		store.add(Event.fromNodeEventEmitter<unknown>(window.webContents, 'zoom-changed')(publish));
		store.add(Event.once(Event.fromNodeEventEmitter<unknown>(window, 'closed'))(() => {
			this._store.delete(store);
			this.revisions.delete(window.id);
		}));
	}
	private window(windowId: number): BrowserWindow {
		const window = this.getWindow();
		if (!window || window.isDestroyed() || window.id !== windowId) {
			throw new Error('Unknown native window');
		}
		return window;
	}
	private state(window: BrowserWindow): IWindowState {
		const revision = (this.revisions.get(window.id) ?? 0) + 1;
		this.revisions.set(window.id, revision);
		return {
			windowId: window.id,
			revision,
			focused: window.isFocused(),
			maximized: window.isMaximized(),
			fullscreen: window.isFullScreen(),
			alwaysOnTop: window.isAlwaysOnTop(),
			zoomFactor: window.webContents.getZoomFactor(),
		};
	}
	public getWindowState(windowId: number): Promise<IWindowState> {
		return Promise.resolve(this.state(this.window(windowId)));
	}
	public focusWindow(windowId: number): Promise<void> {
		const window = this.window(windowId);
		if (window.isMinimized()) {
			window.restore();
		}
		window.focus();
		window.webContents.focus();
		return Promise.resolve();
	}
	public minimizeWindow(windowId: number): Promise<void> {
		this.window(windowId).minimize();
		return Promise.resolve();
	}
	public maximizeWindow(windowId: number): Promise<void> {
		this.window(windowId).maximize();
		return Promise.resolve();
	}
	public unmaximizeWindow(windowId: number): Promise<void> {
		this.window(windowId).unmaximize();
		return Promise.resolve();
	}
	public closeWindow(windowId: number): Promise<void> {
		this.window(windowId).close();
		return Promise.resolve();
	}
	public setFullScreen(windowId: number, fullscreen: boolean): Promise<void> {
		this.window(windowId).setFullScreen(fullscreen);
		return Promise.resolve();
	}
	public updateWindowControls(windowId: number, options: IWindowControlsOptions): Promise<void> {
		new CodeWindow(this.window(windowId)).updateWindowControls(options);
		return Promise.resolve();
	}
}

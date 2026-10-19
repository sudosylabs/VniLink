/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls, window state, folder picking, message boxes, displays, allowlisted links and clipboard.
 *--------------------------------------------------------------------------------------------*/

import { clipboard, ClipboardItem, dialog, screen, shell } from 'electron';
import type { BrowserWindow } from 'electron';
import type { VSBuffer } from '../../../base/common/buffer.js';
import type { MessageBoxOptions, MessageBoxReturnValue } from '../../../base/parts/sandbox/common/electronTypes.js';
import { Disposable, DisposableStore } from '../../../base/common/lifecycle.js';
import { Emitter, Event } from '../../../base/common/event.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { isMacintosh } from '../../../base/common/platform.js';
import type { NativeScreenSource } from '../../engine/common/engine.js';
import { isNativeScreenSource } from '../../engine/common/engine.js';
import type { IWindowState, IWindowControlsOptions } from '../common/native.js';
import { IWindowsMainService } from '../../windows/electron-main/windowsMainService.js';
import type { WindowsMainService } from '../../windows/electron-main/windowsMainService.js';
import { CodeWindow } from '../../windows/electron-main/windowImpl.js';
import { WindowMaterial } from '../../windows/electron-main/windowMaterial.js';

export const INativeHostMainService = createDecorator<NativeHostMainService>('nativeHostMainService');

export class NativeHostMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly stateChanged = this._register(new Emitter<IWindowState>());
	public readonly onDidChangeWindowState = this.stateChanged.event;
	private readonly revisions = new Map<number, number>();
	private readonly materials = new Map<number, WindowMaterial>();

	constructor(@IWindowsMainService private readonly windowsMainService: WindowsMainService) {
		super();
		this._register(windowsMainService.onDidOpenWindow(window => {
			this.registerWindow(window);
		}));
	}

	private registerWindow(window: BrowserWindow): void {
		const store = this._register(new DisposableStore());
		const publish = (): void => {
			if (!window.isDestroyed()) {
				this.stateChanged.fire(this.state(window));
			}
		};
		const material = store.add(new WindowMaterial(window));
		this.materials.set(window.id, material);
		store.add(material.onDidChange(publish));
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
			this.materials.delete(window.id);
		}));
	}

	private window(windowId: number): BrowserWindow {
		const window = this.windowsMainService.getWindowById(windowId);
		if (!window) {
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
			backgroundMaterial: this.materials.get(window.id)?.material ?? 'none',
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
		new CodeWindow(this.window(windowId), this.materials.get(windowId)?.material ?? 'none').updateWindowControls(options);
		return Promise.resolve();
	}

	/** Lets the user choose a folder; resolves `null` when cancelled. */
	public async pickFolder(windowId: number, defaultPath: string): Promise<string | null> {
		const result = await dialog.showOpenDialog(this.window(windowId), { defaultPath, properties: ['openDirectory', 'createDirectory'] });
		return result.canceled ? null : result.filePaths[0] ?? null;
	}

	/** A native message box over the window, as VS Code's dialog service shows one. */
	public async showMessageBox(windowId: number, options: MessageBoxOptions): Promise<MessageBoxReturnValue> {
		const { response, checkboxChecked } = await dialog.showMessageBox(this.window(windowId), {
			message: options.message,
			detail: options.detail,
			type: options.type,
			buttons: options.buttons,
			defaultId: options.defaultId,
			cancelId: options.cancelId,
			title: options.title,
			noLink: options.noLink,
		});
		return { response, checkboxChecked };
	}

	/** Displays that native screen sharing can capture; only macOS has a capture backend. */
	public getCaptureDisplays(windowId: number): Promise<NativeScreenSource[]> {
		this.window(windowId);
		if (!isMacintosh) {
			return Promise.resolve([]);
		}
		return Promise.resolve(screen.getAllDisplays().slice(0, 16).map(display => ({
			displayId: display.id,
			width: display.bounds.width,
			height: display.bounds.height,
			pixelWidth: Math.round(display.bounds.width * display.scaleFactor),
			pixelHeight: Math.round(display.bounds.height * display.scaleFactor),
		})).filter(isNativeScreenSource));
	}

	/** Opens a link from {@link allowedExternalLinks} in the system browser. */
	public async openExternal(windowId: number, url: string): Promise<void> {
		this.window(windowId);
		if (!allowedExternalLinks.includes(url)) {
			throw new Error('Link is not allowed');
		}
		await shell.openExternal(url);
	}

	public async writeClipboardPicture(windowId: number, png: VSBuffer): Promise<void> {
		this.window(windowId);
		await clipboard.write([new ClipboardItem({ 'image/png': new Blob([Uint8Array.from(png.buffer)], { type: 'image/png' }) })]);
	}

	public async readClipboardText(windowId: number, type?: 'selection' | 'clipboard'): Promise<string> {
		this.window(windowId);
		return clipboardFor(type).readText();
	}

	public async writeClipboardText(windowId: number, text: string, type?: 'selection' | 'clipboard'): Promise<void> {
		this.window(windowId);
		return clipboardFor(type).writeText(text);
	}
}

/** Electron exposes the selection clipboard as its own object, on Linux only. */
function clipboardFor(type?: 'selection' | 'clipboard'): Electron.Clipboard {
	// eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- undefined off Linux.
	return type === 'selection' ? clipboard.selection ?? clipboard : clipboard;
}

/** The only pages the workbench may open outside the application. */
export const allowedExternalLinks: readonly string[] = ['https://docs.iroh.computer/relays/rate-limiting'];

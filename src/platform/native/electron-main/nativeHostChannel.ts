import { Event } from '../../../base/common/event.js';
import type { IServerChannel } from '../../../base/parts/ipc/common/ipc.js';
import { windowIdFromContext } from '../../ipc/electron-main/windowContext.js';
import { isNativeHostRequest, type IWindowControlsOptions, type IWindowState } from '../common/native.js';
import type { NativeHostMainService } from './nativeHostMainService.js';

/**
 * Serves `INativeHostService` to one window. Every request names its window
 * first; requests naming any other window are refused.
 */
export class NativeHostChannel implements IServerChannel {

	constructor(
		private readonly nativeHost: NativeHostMainService,
		private readonly relaunch: () => Promise<void>,
	) { }

	public listen<T>(context: string, event: string): Event<T> {
		const windowId = windowIdFromContext(context);
		if (event === 'onDidChangeWindowState') {
			return Event.filter(this.nativeHost.onDidChangeWindowState, (state: IWindowState) => state.windowId === windowId) as Event<T>;
		}
		throw new Error(`Unknown native host event: ${event}`);
	}

	public async call<T>(context: string, command: string, arg?: unknown): Promise<T> {
		const windowId = windowIdFromContext(context);
		const args: unknown[] = Array.isArray(arg) ? arg : [];
		if (!isNativeHostRequest(command, args) || args[0] !== windowId) {
			throw new Error('Invalid native host request');
		}
		return await this.dispatch(command, windowId, args) as T;
	}

	private async dispatch(command: string, windowId: number, args: unknown[]): Promise<unknown> {
		switch (command) {
			case 'getWindowState': return this.nativeHost.getWindowState(windowId);
			case 'focusWindow': return this.nativeHost.focusWindow(windowId);
			case 'minimizeWindow': return this.nativeHost.minimizeWindow(windowId);
			case 'maximizeWindow': return this.nativeHost.maximizeWindow(windowId);
			case 'unmaximizeWindow': return this.nativeHost.unmaximizeWindow(windowId);
			case 'closeWindow': return this.nativeHost.closeWindow(windowId);
			case 'setFullScreen': return this.nativeHost.setFullScreen(windowId, args[1] === true);
			case 'updateWindowControls': return this.nativeHost.updateWindowControls(windowId, args[1] as IWindowControlsOptions);
			case 'pickFolder': return this.nativeHost.pickFolder(windowId, String(args[1]));
			case 'getCaptureDisplays': return this.nativeHost.getCaptureDisplays(windowId);
			case 'openExternal': return this.nativeHost.openExternal(windowId, String(args[1]));
			case 'readClipboardText': return this.nativeHost.readClipboardText(windowId, args[1] as 'selection' | 'clipboard' | undefined);
			case 'writeClipboardText': return this.nativeHost.writeClipboardText(windowId, String(args[1]), args[2] as 'selection' | 'clipboard' | undefined);
			case 'relaunch': return this.relaunch();
		}
		throw new Error('Invalid native host request');
	}
}

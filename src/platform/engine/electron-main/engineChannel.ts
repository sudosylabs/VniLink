import { MessageChannelMain } from 'electron';
import type { WebContents } from 'electron';
import { randomUUID } from 'node:crypto';
import type { Event } from '../../../base/common/event.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { isMacintosh } from '../../../base/common/platform.js';
import type { IServerChannel } from '../../../base/parts/ipc/common/ipc.js';
import { windowIdFromContext } from '../../ipc/electron-main/windowContext.js';
import type { NativeHostMainService } from '../../native/electron-main/nativeHostMainService.js';
import type { PermissionsMainService } from '../../permissions/electron-main/permissionsMainService.js';
import type { WindowsMainService } from '../../windows/electron-main/windowsMainService.js';
import { isChannelRequest } from '../common/dataChannel.js';
import { isEngineCommand, type EngineCommand, type EngineReply } from '../common/engine.js';
import { DATA_CHANNEL_PORT_RESPONSE } from '../common/engineService.js';
import type { EngineMainService } from './engineMainService.js';

/**
 * Engine commands only the main process may send. Windows receive events
 * through `onDidReceiveEvents`, and change settings through the configuration
 * channel, which owns their persistence, validation and read-back.
 */
const privateCommands: ReadonlySet<EngineCommand['type']> = new Set([
	'subscribeEvents', 'pollEvents', 'releaseEvents',
	// Hosting follows `application.acceptConnections`, owned by `HostingMainService`.
	'startHosting', 'stopHosting', 'stopHostedSession',
	'saveCustomRelays', 'setRelayPublic', 'setRelayCustom',
	'setSharingPolicy', 'setSystemPolicy', 'setTerminalPolicy', 'setTerminalShell', 'setPowerPolicy',
	'setDesktopCodecDefault', 'setDesktopQualityDefaults', 'setInputDefaults',
	'setNativeCameraSources', 'setNativeAudioSources', 'setFileRoots', 'enableFilesystemAccess',
]);

/**
 * Serves engine commands and data channels to windows. Commands are validated
 * here even though the renderer and Rust validate them too, and data channels
 * belong to the window that opened them until it navigates or goes away.
 */
export class EngineChannel extends Disposable implements IServerChannel {
	private readonly owners = new Map<number, { readonly id: string; readonly release: () => void }>();

	constructor(
		private readonly engine: EngineMainService,
		private readonly windows: WindowsMainService,
		private readonly nativeHost: NativeHostMainService,
		private readonly permissions: PermissionsMainService,
	) {
		super();
		this._register(toDisposable(() => {
			for (const owner of [...this.owners.values()]) {
				owner.release();
			}
		}));
	}

	public listen<T>(_context: string, event: string): Event<T> {
		switch (event) {
			case 'onDidChangeAvailability': return this.engine.onDidChangeAvailability as Event<T>;
			case 'onDidChangeRestartRequired': return this.engine.onDidChangeRestartRequired as Event<T>;
			case 'onDidReceiveEvents': return this.engine.onDidReceiveEvents as Event<T>;
		}
		throw new Error(`Unknown engine event: ${event}`);
	}

	public async call<T>(context: string, command: string, arg?: unknown): Promise<T> {
		const windowId = windowIdFromContext(context);
		switch (command) {
			case 'getAvailability': return this.engine.available as T;
			case 'getRestartRequired': return await this.engine.getRestartRequired() as T;
			case 'execute': return await this.execute(arg) as T;
			case 'openDataChannel':
				this.openDataChannel(windowId, arg);
				return undefined as T;
		}
		throw new Error(`Unknown engine command: ${command}`);
	}

	private async execute(command: unknown): Promise<EngineReply> {
		if (!isEngineCommand(command) || privateCommands.has(command.type)) {
			throw new Error('Invalid engine command');
		}
		if (command.type === 'startNativeScreenSharing') {
			await this.checkScreenSharing(command);
		}
		return this.engine.execute(command);
	}

	private async checkScreenSharing(command: Extract<EngineCommand, { type: 'startNativeScreenSharing' }>): Promise<void> {
		if (!isMacintosh || !this.permissions.isGranted('screen')) {
			throw new Error('Screen sharing requires a supported native backend and Screen Recording permission.');
		}
		const window = this.windows.getMainWindow();
		const displays = window ? await this.nativeHost.getCaptureDisplays(window.id) : [];
		const display = displays.find(candidate => candidate.displayId === command.displayId);
		if (display?.width !== command.width || display.height !== command.height
			|| display.pixelWidth !== command.pixelWidth || display.pixelHeight !== command.pixelHeight) {
			throw new Error('Display geometry changed. Reload display selection before sharing.');
		}
		await this.permissions.synchronize();
	}

	private openDataChannel(windowId: number, request: unknown): void {
		const window = this.windows.getWindowById(windowId);
		if (!window || !isChannelRequest(request)) {
			throw new Error('Invalid data channel request');
		}
		const owner = this.ownerFor(windowId, window.webContents);
		const { port1, port2 } = new MessageChannelMain();
		try {
			this.engine.attachChannel(port1, request, owner);
			window.webContents.postMessage(DATA_CHANNEL_PORT_RESPONSE, request.requestId, [port2]);
		} catch (error) {
			port1.close();
			port2.close();
			throw error;
		}
	}

	/** Returns the window's channel owner, created on first use and released with the page. */
	private ownerFor(windowId: number, contents: WebContents): string {
		const existing = this.owners.get(windowId);
		if (existing) {
			return existing.id;
		}
		const id = randomUUID();
		const navigate = (_event: Electron.Event, _url: string, _inPlace: boolean, mainFrame: boolean): void => {
			if (mainFrame) {
				release();
			}
		};
		const release = (): void => {
			if (this.owners.get(windowId)?.id === id) {
				this.engine.closeChannels(id);
				this.owners.delete(windowId);
			}
			// Native-backed listener removal is invalid after WebContents destruction.
			if (!contents.isDestroyed()) {
				contents.removeListener('destroyed', release);
				contents.removeListener('render-process-gone', release);
				contents.removeListener('did-start-navigation', navigate);
			}
		};
		this.owners.set(windowId, { id, release });
		contents.once('destroyed', release);
		contents.once('render-process-gone', release);
		contents.on('did-start-navigation', navigate);
		return id;
	}
}

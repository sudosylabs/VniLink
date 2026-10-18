import { app, Menu, nativeImage, nativeTheme, Tray } from 'electron';
import type { MenuItemConstructorOptions } from 'electron';
import { join } from 'node:path';
import { Emitter, Event } from '../../../base/common/event.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { isMacintosh, isWindows } from '../../../base/common/platform.js';
import * as nls from '../../../nls.js';
import { IHostingMainService, type HostingMainService, type HostingStatus } from '../../hosting/electron-main/hostingMainService.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';
import { product } from '../../product/common/product.js';

export const ITrayMainService = createDecorator<TrayMainService>('trayMainService');

/** Matches the artwork rendered by `build/lib/trayAssets.ts`. */
export type TrayState = 'idle' | 'active' | 'paused';

export type TrayAction = { readonly kind: 'open' | 'quit' | 'pause' | 'resume' } | { readonly kind: 'disconnect'; readonly sessionId: string };

/** A menu row before Electron sees it, so the menu can be compared and tested. */
export type TrayMenuEntry = { readonly type: 'separator' } | { readonly label: string; readonly enabled: boolean; readonly action?: TrayAction };

/** Someone connected outranks everything; anything that stops devices reaching this computer shows as paused. */
export function trayState(status: HostingStatus): TrayState {
	if (status.sessions.length) {
		return 'active';
	}
	return status.available && status.accepting && status.hosting ? 'idle' : 'paused';
}

/** One line saying whether paired devices can reach this computer, and who has. */
export function trayStatusLine(status: HostingStatus): string {
	if (!status.available) {
		return nls.localize('tray.unavailable', 'Connection engine unavailable');
	}
	if (!status.accepting) {
		return nls.localize('tray.paused', 'Not accepting connections');
	}
	if (status.sessions.length === 1) {
		return nls.localize('tray.connectedOne', '1 device connected');
	}
	if (status.sessions.length > 1) {
		return nls.localize('tray.connectedMany', '{0} devices connected', status.sessions.length);
	}
	return status.hosting ? nls.localize('tray.ready', 'Ready for paired devices') : nls.localize('tray.waiting', 'Waiting to accept connections');
}

export function trayMenu(status: HostingStatus): TrayMenuEntry[] {
	const entries: TrayMenuEntry[] = [{ label: trayStatusLine(status), enabled: false }];
	if (status.sessions.length) {
		entries.push({ type: 'separator' }, { label: nls.localize('tray.connected', 'Connected to this computer'), enabled: false });
		for (const session of status.sessions) {
			const label = session.phase === 'Active'
				? nls.localize('tray.disconnect', 'Disconnect {0}', session.name)
				: nls.localize('tray.disconnectConnecting', 'Disconnect {0} (connecting)', session.name);
			entries.push({ label, enabled: true, action: { kind: 'disconnect', sessionId: session.id } });
		}
	}
	if (status.available) {
		entries.push({ type: 'separator' }, status.accepting
			? { label: nls.localize('tray.pause', 'Stop Accepting Connections'), enabled: true, action: { kind: 'pause' } }
			: { label: nls.localize('tray.resume', 'Accept Connections'), enabled: true, action: { kind: 'resume' } });
	}
	entries.push(
		{ type: 'separator' },
		{ label: nls.localize('tray.open', 'Open {0}', product.name), enabled: true, action: { kind: 'open' } },
		{ label: nls.localize('tray.quit', 'Quit {0}', product.name), enabled: true, action: { kind: 'quit' } },
	);
	return entries;
}

/**
 * Which artwork variant fits the panel: macOS template images take the menu
 * bar's tint; Windows follows the taskbar's theme; Linux panels are usually
 * dark and Electron cannot read their color.
 */
export function trayImageName(state: TrayState, platform: 'darwin' | 'win32' | 'linux', darkPanel: boolean): string {
	if (platform === 'darwin') {
		return `${state}Template.png`;
	}
	return `${state}-${platform === 'linux' || darkPanel ? 'dark' : 'light'}.png`;
}

/**
 * The menu bar extra or notification-area icon. It shows whether paired
 * devices can reach this computer, lets the owner disconnect each one or stop
 * accepting connections, and is how a windowless application is reopened or
 * quit.
 */
export class TrayMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly windowRequested = this._register(new Emitter<void>());
	/** The owner asked to see the window. */
	public readonly onDidRequestWindow = this.windowRequested.event;
	private tray: Tray | undefined;
	private shownImage: string | undefined;
	private shownMenu: string | undefined;

	constructor(
		@IHostingMainService private readonly hosting: HostingMainService,
		@ILogService private readonly logService: ILogService,
	) {
		super();
	}

	public create(): void {
		if (this.tray || this._store.isDisposed) {
			return;
		}
		const tray = new Tray(this.image(trayState(this.hosting.status)));
		this.tray = tray;
		this._register(toDisposable(() => {
			tray.destroy();
		}));
		if (!isMacintosh) {
			// macOS opens the menu on click; Windows and Linux keep the menu for the secondary button.
			this._register(Event.fromNodeEventEmitter<unknown>(tray, 'click')(() => {
				this.windowRequested.fire();
			}));
		}
		this._register(this.hosting.onDidChangeStatus(() => {
			this.render();
		}));
		this._register(Event.fromNodeEventEmitter<unknown>(nativeTheme, 'updated')(() => {
			this.render();
		}));
		this.render();
	}

	private render(): void {
		const tray = this.tray;
		if (!tray || tray.isDestroyed()) {
			return;
		}
		const status = this.hosting.status;
		const name = this.imageName(trayState(status));
		if (name !== this.shownImage) {
			tray.setImage(this.image(trayState(status)));
		}
		tray.setToolTip(`${product.name} – ${trayStatusLine(status)}`);
		const entries = trayMenu(status);
		const key = JSON.stringify(entries);
		if (key !== this.shownMenu) {
			this.shownMenu = key;
			tray.setContextMenu(Menu.buildFromTemplate(entries.map(entry => this.item(entry))));
		}
	}

	private item(entry: TrayMenuEntry): MenuItemConstructorOptions {
		if ('type' in entry) {
			return { type: 'separator' };
		}
		const action = entry.action;
		if (!action) {
			return { label: entry.label, enabled: entry.enabled };
		}
		return {
			label: entry.label, enabled: entry.enabled, click: () => {
				this.run(action);
			},
		};
	}

	private run(action: TrayAction): void {
		switch (action.kind) {
			case 'open':
				this.windowRequested.fire();
				return;
			case 'quit':
				app.quit();
				return;
			case 'pause':
			case 'resume':
				this.hosting.setAccepting(action.kind === 'resume').catch((error: unknown) => {
					this.logService.error('The connection choice could not be saved.', error);
				});
				return;
			case 'disconnect':
				this.hosting.disconnect(action.sessionId).catch((error: unknown) => {
					this.logService.error('The device could not be disconnected.', error);
				});
				return;
		}
	}

	private imageName(state: TrayState): string {
		return trayImageName(state, isMacintosh ? 'darwin' : isWindows ? 'win32' : 'linux', nativeTheme.shouldUseDarkColorsForSystemIntegratedUI);
	}

	private image(state: TrayState): Electron.NativeImage {
		const name = this.imageName(state);
		this.shownImage = name;
		// Electron loads the @1.5x and @2x representations beside this file.
		return nativeImage.createFromPath(join(__dirname, 'assets', 'tray', name));
	}
}

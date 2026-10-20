import { verifyDevices, verifyDeviceScreenshot, verifyPairingPopover, DevicePageFixture, type DeviceFixtureState } from './devices.js';
import { verifyMediaSettings } from './mediaSettings.js';
/** Independently selectable renderer scenarios; each owns its fixture teardown. */
import { verifyWorkbenchLifecycle } from './workbench.js';
import { verifyAccess } from './access.js';
import { verifyCamera } from './camera.js';
import { verifyConnection } from './connection.js';
import { ContextMenuFixture, verifyContextMenu } from './contextMenu.js';
import { verifyDataChannels } from './dataChannels.js';
import { showThemedDialog, verifyDialogs } from './dialogs.js';
import { showFilesFixture, verifyFiles } from './files.js';
import { verifyHomeLifecycle } from './home.js';
import { verifyIcons } from './icons.js';
import { verifyRemoteDesktop } from './remoteDesktop.js';
import { verifyRemoteDesktopSession } from './remoteDesktopSession.js';
import { verifySettingsPages } from './settings.js';
import { verifyTabs } from './tabs.js';
import { verifyTerminal } from './terminal.js';

export class BrowserScenarios {
	private deviceFixture: DevicePageFixture | undefined;
	private contextMenuFixture: ContextMenuFixture | undefined;

	public showDeviceFixture(state: DeviceFixtureState): void {
		this.clearDeviceFixture();
		this.deviceFixture = new DevicePageFixture(state);
	}

	/** Opens the fixture's create-invitation popover, as the titlebar action does. */
	public shareFixtureInvitation(): void {
		this.deviceFixture?.anchor('create').click();
	}

	public requestFixturePairing(): void {
		if (this.deviceFixture) {
			this.deviceFixture.incoming = { endpoint_id: 'test-computer', check: '731' };
		}
	}

	/** Opens the themed restart confirmation; `closeDialogFixture` answers it with Escape. */
	public showDialogFixture(): void {
		void showThemedDialog();
	}

	public closeDialogFixture(): void {
		for (const type of ['keydown', 'keyup']) {
			window.dispatchEvent(new KeyboardEvent(type, { key: 'Escape', code: 'Escape', keyCode: 27 }));
		}
	}

	/** Opens a context menu for screenshots; `closeContextMenuFixture` cancels it with Escape. */
	public showContextMenuFixture(): void {
		this.contextMenuFixture?.dispose();
		this.contextMenuFixture = new ContextMenuFixture();
		this.contextMenuFixture.show();
	}

	public closeContextMenuFixture(): void {
		for (const type of ['keydown', 'keyup']) {
			document.activeElement?.dispatchEvent(new KeyboardEvent(type, { key: 'Escape', code: 'Escape', keyCode: 27, bubbles: true }));
		}
		this.contextMenuFixture?.dispose();
		this.contextMenuFixture = undefined;
	}

	private filesFixture: { dispose(): void } | undefined;

	/** Shows the Files tab mid-task at `width`×`height`; `clearFilesFixture` removes it. */
	public async showFilesFixture(width: number, height: number): Promise<void> {
		this.clearFilesFixture();
		this.filesFixture = await showFilesFixture(width, height);
	}

	public clearFilesFixture(): void {
		this.filesFixture?.dispose();
		this.filesFixture = undefined;
	}

	public clearDeviceFixture(): void {
		this.deviceFixture?.dispose();
		this.deviceFixture = undefined;
	}

	private readonly scenarios = new Map<string, () => unknown>([
		['workbench', verifyWorkbenchLifecycle], ['access', verifyAccess], ['camera', verifyCamera], ['connection', verifyConnection], ['context-menu', verifyContextMenu], ['data-channels', verifyDataChannels],
		['devices', verifyDevices], ['dialogs', verifyDialogs], ['files', verifyFiles], ['device-pairing', verifyPairingPopover], ['device-screenshot', verifyDeviceScreenshot], ['home', verifyHomeLifecycle], ['icons', verifyIcons],
		['media-settings', verifyMediaSettings], ['remote-desktop', verifyRemoteDesktop], ['remote-desktop-session', verifyRemoteDesktopSession], ['settings', verifySettingsPages], ['tabs', verifyTabs], ['terminal', verifyTerminal],
	]);

	public async run(name: string): Promise<boolean> {
		const scenario = this.scenarios.get(name);
		if (!scenario) {
			throw new Error(`Unknown browser scenario: ${name}`);
		}
		await scenario();
		return true;
	}
}

export const browserScenarios = new BrowserScenarios();

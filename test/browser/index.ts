import { verifyDevices, verifyDeviceScreenshot, verifyPairingPopover, DevicePageFixture, type DeviceFixtureState } from './devices.js';
import { verifyMediaSettings } from './mediaSettings.js';
/** Independently selectable renderer scenarios; each owns its fixture teardown. */
import { verifyWorkbenchLifecycle } from './workbench.js';
import { verifyAccess } from './access.js';
import { verifyCamera } from './camera.js';
import { verifyConnection } from './connection.js';
import { verifyDataChannels } from './dataChannels.js';
import { showThemedDialog, verifyDialogs } from './dialogs.js';
import { verifyHomeLifecycle } from './home.js';
import { verifyIcons } from './icons.js';
import { verifyRemoteDesktop } from './remoteDesktop.js';
import { verifySettingsPages } from './settings.js';
import { verifyTabs } from './tabs.js';
import { verifyTerminal } from './terminal.js';

export class BrowserScenarios {
	private deviceFixture: DevicePageFixture | undefined;

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

	public clearDeviceFixture(): void {
		this.deviceFixture?.dispose();
		this.deviceFixture = undefined;
	}

	private readonly scenarios = new Map<string, () => unknown>([
		['workbench', verifyWorkbenchLifecycle], ['access', verifyAccess], ['camera', verifyCamera], ['connection', verifyConnection], ['data-channels', verifyDataChannels],
		['devices', verifyDevices], ['dialogs', verifyDialogs], ['device-pairing', verifyPairingPopover], ['device-screenshot', verifyDeviceScreenshot], ['home', verifyHomeLifecycle], ['icons', verifyIcons],
		['media-settings', verifyMediaSettings], ['remote-desktop', verifyRemoteDesktop], ['settings', verifySettingsPages], ['tabs', verifyTabs], ['terminal', verifyTerminal],
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

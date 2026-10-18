import { verifyDevices, verifyDeviceScreenshot, DevicePageFixture, type DeviceFixtureState } from './devices.js';
import { verifyMediaSettings } from './mediaSettings.js';
/** Independently selectable renderer scenarios; each owns its fixture teardown. */
import { verifyWorkbenchLifecycle } from './workbench.js';
import { verifyAccess } from './access.js';
import { verifyConnection } from './connection.js';
import { verifyDataChannels } from './dataChannels.js';
import { verifyHomeLifecycle } from './home.js';
import { verifyIcons } from './icons.js';
import { verifyRemoteDesktop } from './remoteDesktop.js';
import { verifySettingsPages } from './settings.js';
import { verifyTabs } from './tabs.js';

export class BrowserScenarios {
	private deviceFixture: DevicePageFixture | undefined;

	public showDeviceFixture(state: DeviceFixtureState): void {
		this.clearDeviceFixture();
		this.deviceFixture = new DevicePageFixture(state);
	}

	public shareFixtureInvitation(): void {
		this.deviceFixture?.page.shareInvitation();
	}

	public requestFixturePairing(): void {
		if (this.deviceFixture) {
			this.deviceFixture.incoming = { endpoint_id: 'test-computer', check: '731' };
		}
	}

	public clearDeviceFixture(): void {
		this.deviceFixture?.dispose();
		this.deviceFixture = undefined;
	}

	private readonly scenarios = new Map<string, () => unknown>([
		['workbench', verifyWorkbenchLifecycle], ['access', verifyAccess], ['connection', verifyConnection], ['data-channels', verifyDataChannels],
		['devices', verifyDevices], ['device-screenshot', verifyDeviceScreenshot], ['home', verifyHomeLifecycle], ['icons', verifyIcons],
		['media-settings', verifyMediaSettings], ['remote-desktop', verifyRemoteDesktop], ['settings', verifySettingsPages], ['tabs', verifyTabs],
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

/** Independently selectable renderer scenarios; each owns its fixture teardown. */
import { verifyWorkbenchLifecycle } from './workbench.js';
import { verifyAccess } from './access.js';
import { verifyDataChannels } from './dataChannels.js';
import { verifyFileTerminalSettings } from './fileTerminalSettings.js';
import { verifyHomeLifecycle } from './home.js';
import { verifyIcons } from './icons.js';
import { verifyInputPage } from './input.js';
import { verifyMachinePermissions } from './permissions.js';
import { verifyRemoteDesktop } from './remoteDesktop.js';
import { verifySettingsPages } from './settings.js';
import { verifyTabs } from './tabs.js';

export class BrowserScenarios {
	private readonly scenarios = new Map<string, () => unknown>([
		['workbench', verifyWorkbenchLifecycle], ['access', verifyAccess], ['data-channels', verifyDataChannels], ['files-terminal', verifyFileTerminalSettings],
		['home', verifyHomeLifecycle], ['icons', verifyIcons], ['input', verifyInputPage],
		['permissions', verifyMachinePermissions], ['remote-desktop', verifyRemoteDesktop], ['settings', verifySettingsPages], ['tabs', verifyTabs],
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

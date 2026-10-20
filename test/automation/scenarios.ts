/** Validated selectors shared by the CLI launcher and Electron suite. */
export const browserScenarioNames = ['workbench', 'access', 'camera', 'connection', 'context-menu', 'data-channels', 'devices', 'dialogs', 'files', 'device-pairing', 'device-screenshot', 'home', 'icons', 'media-settings', 'remote-desktop', 'remote-desktop-session', 'settings', 'tabs', 'terminal'];

export const integrationScenarioNames = ['engine', 'navigation', 'window', 'appearance', 'settings-layout', 'reload', 'notice'];

export const scenarioNames = [...browserScenarioNames, ...integrationScenarioNames];

export class ScenarioSelection {
	public names: string[];
	public list: boolean;

	constructor(args: readonly string[]) {
		this.names = [];
		this.list = false;
		for (let index = 0; index < args.length; index++) {
			const value = args[index];
			if (value === '--list') {
				this.list = true;
			} else if (value === '--scenario') {
				const name = args[++index];
				if (!name) {
					throw new Error('--scenario requires a name');
				}
				this.names.push(name);
			} else if (value.startsWith('--scenario=')) {
				this.names.push(value.slice('--scenario='.length));
			} else {
				throw new Error(`Unknown test option: ${value}`);
			}
		}
		for (const name of this.names) {
			if (!scenarioNames.includes(name)) {
				throw new Error(`Unknown scenario: ${name}`);
			}
		}
		this.names = [...new Set(this.names.length ? this.names : scenarioNames)];
	}

	public includes(name: string) {
		return this.names.includes(name);
	}
}

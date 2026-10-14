/** Validated selectors shared by the CLI launcher and Electron suite. */
export const browserScenarioNames = ['workbench', 'access', 'data-channels', 'files-terminal', 'home', 'icons', 'input', 'permissions', 'remote-desktop', 'settings', 'tabs'];

export const integrationScenarioNames = ['engine', 'navigation', 'window', 'appearance', 'reload'];

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

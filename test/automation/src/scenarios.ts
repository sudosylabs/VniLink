// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Validated selectors shared by the CLI launcher and Electron suite. The
 * browser scenarios load the window's smoke fixtures (`test/smoke/fixtures`):
 * `data-channels` checks the real engine's channels, and the others lay out
 * and photograph a fixture. Renderer behaviour is tested by the unit suites.
 */
export const browserScenarioNames = ['context-menu', 'data-channels', 'devices', 'dialogs', 'files', 'remote-desktop-session'];

export const integrationScenarioNames = ['engine', 'navigation', 'window', 'window-theme', 'appearance', 'settings-layout', 'reload', 'notice', 'engine-restart'];

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

	/**
	 * The smoke suite's areas to run, as a mocha `grep`: each selected
	 * scenario is the area of that name, and every run checks the renderer.
	 */
	public get pattern(): RegExp {
		return new RegExp(`^(?:${['renderer', ...this.names].join('|')}) `);
	}
}

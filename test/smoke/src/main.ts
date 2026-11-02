// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The smoke suite: the real application in a temporary profile, driven
 * through its window. Each area is a mocha suite named after its scenario
 * (`test/automation/src/scenarios.ts`); `--scenario <name>` selects areas.
 */
import Mocha from 'mocha';
import { TestApplication } from '../../automation/src/application.js';
import { Automation } from '../../automation/src/automation.js';
import { ScenarioSelection } from '../../automation/src/scenarios.js';
import { setup as setupDevices } from './areas/devices/devices.test.js';
import { setup as setupEngine } from './areas/engine/engine.test.js';
import { setup as setupFixtures } from './areas/fixtures/fixtures.test.js';
import { setup as setupNotice } from './areas/notice/notice.test.js';
import { setup as setupRenderer } from './areas/renderer/renderer.test.js';
import { setup as setupWorkbench } from './areas/workbench/workbench.test.js';

// Each test has mocha's minute; the whole run, its areas on the slowest computer it runs on
// (the Linux laptop takes minutes where a Mac takes seconds), has ten.
const application = new TestApplication({ timeoutMs: 10 * 60_000 });

const selection = new ScenarioSelection(application.args);

void application.run(async (window, driver) => {
	const automation = new Automation(window, driver, application);
	const mocha = new Mocha({ ui: 'bdd', reporter: 'spec', timeout: 60_000, grep: selection.pattern });
	// The areas call mocha's `describe` and `it`, which belong to this run once it is announced.
	mocha.suite.emit('pre-require', globalThis, 'smoke', mocha);
	setupRenderer(automation);
	setupFixtures(automation);
	setupDevices(automation);
	setupNotice(automation);
	setupEngine(automation);
	setupWorkbench(automation);
	const failures = await new Promise<number>(resolve => mocha.run(resolve));
	if (failures) {
		throw new Error(`${String(failures)} smoke ${failures === 1 ? 'test' : 'tests'} failed`);
	}
	console.log(`PASS: ${selection.names.join(', ')}`);
});

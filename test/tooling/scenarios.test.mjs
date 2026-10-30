// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { ScenarioSelection } from '../automation/scenarios.ts';

test('scenario selectors are independent, deduplicated and reject invalid input', () => {
	assert.deepEqual(new ScenarioSelection(['--scenario', 'tabs', '--scenario=settings', '--scenario', 'tabs']).names, ['tabs', 'settings']);
	assert.throws(() => new ScenarioSelection(['--scenario']), /requires/);
	assert.throws(() => new ScenarioSelection(['--scenario', 'not-real']), /Unknown scenario/);
	assert.throws(() => new ScenarioSelection(['--bad']), /Unknown test option/);
});

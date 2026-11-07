// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../base/test/common/utils.js';
import { product } from '../../../platform/product/common/product.js';
import { planLaunch, type LaunchContext } from '../../electron-main/launch.js';

/** A launch of the source checkout on macOS with `argv`; `overrides` set what a test cares about. */
function context(argv: readonly string[], overrides: Partial<LaunchContext> = {}): LaunchContext {
	return { argv: ['vnilink', ...argv], built: false, windows: false, hasSwitch: () => false, testProfile: undefined, ...overrides };
}

suite('planLaunch', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('gives Windows the application id', () => {
		assert.strictEqual(planLaunch(context([], { windows: true })).appUserModelId, product.appId);
	});
	test('gives other platforms no application id', () => {
		assert.strictEqual(planLaunch(context([])).appUserModelId, undefined);
	});
	test('matches a built application to its installed desktop entry', () => {
		assert.strictEqual(planLaunch(context([], { built: true })).desktopName, product.linux.desktopFileName);
	});
	test('gives the source checkout a desktop entry of its own', () => {
		assert.strictEqual(planLaunch(context([])).desktopName, `${product.appId}.dev.desktop`);
	});
	test('starts the installed engine in the default profile', () => {
		assert.deepStrictEqual(planLaunch(context([])).action, { kind: 'start', userData: undefined, options: { temporaryEngine: false, automationDriver: false } });
	});
	test('refuses a smoke launch without an isolated profile', () => {
		const action = planLaunch(context(['--smoke-test'])).action;
		assert.strictEqual(action.kind, 'invalid');
		assert.match(action.reason, /requires an isolated/);
	});
	test('runs a smoke launch in its profile with a temporary engine and the automation driver', () => {
		assert.deepStrictEqual(planLaunch(context(['--smoke-test'], { testProfile: '/temporary/profile' })).action, {
			kind: 'start', userData: '/temporary/profile', options: { temporaryEngine: true, automationDriver: true },
		});
	});
	test('runs a self-check with its report directory', () => {
		assert.deepStrictEqual(planLaunch(context(['--self-check=/temporary/report'])).action, { kind: 'selfCheck', output: '/temporary/report' });
	});
	const refused: readonly (readonly [argument: string, argv: readonly string[], switches: readonly string[]])[] = [
		['--smoke-test', ['--smoke-test'], []],
		['remote-debugging-port', [], ['remote-debugging-port']],
		['remote-debugging-pipe', [], ['remote-debugging-pipe']],
	];
	for (const [argument, argv, switches] of refused) {
		test(`a built application refuses ${argument} before it starts anything`, () => {
			const plan = planLaunch(context(argv, { built: true, testProfile: '/temporary/profile', hasSwitch: name => switches.includes(name) }));
			assert.deepStrictEqual(plan.action, { kind: 'refuse', argument });
		});
	}
	test('the source checkout accepts remote debugging', () => {
		const plan = planLaunch(context([], { hasSwitch: name => name === 'remote-debugging-port' }));
		assert.strictEqual(plan.action.kind, 'start');
	});
});

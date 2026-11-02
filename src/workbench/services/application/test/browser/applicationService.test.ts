// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { DisposableStore } from '../../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { IEngineService } from '../../../../../platform/engine/common/engineService.js';
import { TestEngineService } from '../../../../../platform/engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { INativeHostService } from '../../../../../platform/native/common/native.js';
import { ApplicationService } from '../../applicationServiceImpl.js';

suite('ApplicationService', () => {
	const disposables = new DisposableStore();
	let engine: TestEngineService;
	/** What the service asked the native host to do, in order. */
	let native: string[];
	function createService(): ApplicationService {
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IEngineService, engine);
		instantiationService.stub(INativeHostService, {
			relaunch: () => {
				native.push('relaunch');
				return Promise.resolve();
			},
			openExternal: (url: string) => {
				native.push(url);
				return Promise.resolve();
			},
		});
		return disposables.add(instantiationService.createInstance(ApplicationService));
	}
	setup(() => {
		engine = disposables.add(new TestEngineService(command => {
			throw new Error(`Unexpected command ${command.type}`);
		}));
		native = [];
	});
	teardown(() => {
		disposables.clear();
	});
	test('restart is not required until the engine says so', () => {
		const service = createService();
		assert.strictEqual(service.restartRequired, false);
		engine.setRestartRequired(true);
		assert.strictEqual(service.restartRequired, true);
	});
	test('a change in the engine\'s restart state is announced once', () => {
		const service = createService();
		let changes = 0;
		disposables.add(service.onDidChangeRestartRequired(() => changes++));
		engine.setRestartRequired(true);
		assert.strictEqual(changes, 1);
	});
	test('restarting relaunches through the native host', async () => {
		await createService().restart();
		assert.deepStrictEqual(native, ['relaunch']);
	});
	test('the relay documentation opens through the native host', async () => {
		await createService().openRelayDocumentation();
		assert.deepStrictEqual(native, ['https://docs.iroh.computer/relays/rate-limiting']);
	});
	test('a disposed service stops following the engine', () => {
		const service = createService();
		let changes = 0;
		disposables.add(service.onDidChangeRestartRequired(() => changes++));
		service.dispose();
		engine.setRestartRequired(false);
		assert.strictEqual(changes, 0);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

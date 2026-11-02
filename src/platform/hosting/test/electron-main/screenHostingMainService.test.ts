// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../base/common/async.js';
import { Emitter } from '../../../../base/common/event.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { IConfigurationMainService } from '../../../configuration/electron-main/configurationMainService.js';
import { TestConfigurationMainService } from '../../../configuration/test/common/testConfigurationMainService.js';
import type { NativeScreenSource } from '../../../engine/common/engine.js';
import { IEngineMainService } from '../../../engine/electron-main/engineMainService.js';
import { TestEngineService } from '../../../engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../log/common/log.js';
import { IPermissionsMainService, type PermissionsMainService } from '../../../permissions/electron-main/permissionsMainService.js';
import { ScreenHostingMainService, type ScreenHostingEnvironment } from '../../electron-main/screenHostingMainService.js';

const SHARING = 'sharing.screen';

const primary: NativeScreenSource = { displayId: 1, width: 1440, height: 900, pixelWidth: 2880, pixelHeight: 1800 };

/** A computer's platform and displays; `changeDisplay` changes the primary display and announces it. */
class TestScreenEnvironment implements ScreenHostingEnvironment {
	private listener: (() => void) | undefined;
	public display = primary;

	constructor(public readonly platform: ScreenHostingEnvironment['platform']) { }

	/** Whether the service follows display changes. */
	public get followsDisplays(): boolean {
		return this.listener !== undefined;
	}

	public primaryDisplay(): NativeScreenSource {
		return this.display;
	}

	public onDidChangeDisplays(listener: () => void): () => void {
		this.listener = listener;
		return () => {
			this.listener = undefined;
		};
	}

	public changeDisplay(display: NativeScreenSource): void {
		this.display = display;
		this.listener?.();
	}
}

/** Screen Recording consent, which only macOS asks for; `grant` gives it and announces the change. */
class TestPermissions implements Pick<PermissionsMainService, 'isGranted' | 'onDidChangeConsent'> {
	private granted = false;

	constructor(private readonly consent: Emitter<void>) { }

	public readonly onDidChangeConsent = this.consent.event;

	public isGranted(kind: string): boolean {
		return kind === 'screen' && this.granted;
	}

	public grant(): void {
		this.granted = true;
		this.consent.fire();
	}
}

suite('ScreenHostingMainService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let engine: TestEngineService;
	let permissions: TestPermissions;
	let configuration: TestConfigurationMainService;
	setup(() => {
		engine = disposables.add(new TestEngineService(() => ({ type: 'done' })));
		permissions = new TestPermissions(disposables.add(new Emitter<void>()));
		configuration = disposables.add(new TestConfigurationMainService({ [SHARING]: false }));
	});
	async function startService(environment: TestScreenEnvironment): Promise<ScreenHostingMainService> {
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IEngineMainService, engine);
		instantiationService.stub(IPermissionsMainService, permissions);
		instantiationService.stub(IConfigurationMainService, configuration);
		instantiationService.stub(ILogService, disposables.add(new NullLogService()));
		const service = disposables.add(instantiationService.createInstance(ScreenHostingMainService, environment));
		await service.start();
		return service;
	}
	function commands(): string[] {
		return engine.commands.map(command => command.type);
	}
	suite('macOS', () => {
		let environment: TestScreenEnvironment;
		setup(() => {
			environment = new TestScreenEnvironment('darwin');
		});
		test('shares nothing without Screen Recording', async () => {
			await startService(environment);
			assert.deepStrictEqual(commands(), []);
		});
		test('hosts the primary display once Screen Recording is granted', async () => {
			await startService(environment);
			permissions.grant();
			await timeout(0);
			assert.deepStrictEqual(engine.commands, [{ type: 'startNativeScreenSharing', ...primary }]);
		});
		test('a display change that leaves the primary display as it was does not restart the host', async () => {
			permissions.grant();
			await startService(environment);
			environment.changeDisplay({ ...primary });
			await timeout(0);
			assert.deepStrictEqual(commands(), ['startNativeScreenSharing']);
		});
		test('a new primary display restarts the host on it', async () => {
			permissions.grant();
			await startService(environment);
			environment.changeDisplay({ ...primary, displayId: 2 });
			await timeout(0);
			assert.deepStrictEqual(engine.commands, [{ type: 'startNativeScreenSharing', ...primary }, { type: 'stopNativeScreenSharing' }, { type: 'startNativeScreenSharing', ...primary, displayId: 2 }]);
		});
		test('a restarted engine is given its host again', async () => {
			permissions.grant();
			await startService(environment);
			engine.setAvailable(false);
			engine.setAvailable(true);
			await timeout(0);
			assert.deepStrictEqual(commands(), ['startNativeScreenSharing', 'startNativeScreenSharing']);
		});
	});
	test('Windows hosts without waiting for a consent it does not have', async () => {
		await startService(new TestScreenEnvironment('win32'));
		assert.deepStrictEqual(commands(), ['startNativeScreenSharing']);
	});
	suite('Linux', () => {
		let environment: TestScreenEnvironment;
		setup(() => {
			environment = new TestScreenEnvironment('linux');
		});
		test('shows no portal dialog while screen sharing is off', async () => {
			await startService(environment);
			assert.deepStrictEqual(commands(), []);
		});
		test('asks the portal once screen sharing is turned on', async () => {
			await startService(environment);
			configuration.setValue(SHARING, true);
			await timeout(0);
			assert.deepStrictEqual(commands(), ['startNativeScreenSharing']);
		});
		test('does not follow displays, which would show the portal dialog again', async () => {
			await startService(environment);
			assert.strictEqual(environment.followsDisplays, false);
		});
		test('turning screen sharing off gives up the portal grant', async () => {
			configuration.setValue(SHARING, true);
			await startService(environment);
			configuration.setValue(SHARING, false);
			await timeout(0);
			assert.deepStrictEqual(commands(), ['startNativeScreenSharing', 'stopNativeScreenSharing']);
		});
	});
	test('a platform without a native host is never asked to share', async () => {
		await startService(new TestScreenEnvironment('other'));
		assert.deepStrictEqual(commands(), []);
	});
	test('a disposed host stops following displays', async () => {
		const environment = new TestScreenEnvironment('darwin');
		const service = await startService(environment);
		service.dispose();
		assert.strictEqual(environment.followsDisplays, false);
	});
});

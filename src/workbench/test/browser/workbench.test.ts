// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { DeferredPromise } from '../../../base/common/async.js';
import { $ } from '../../../base/browser/dom.js';
import type { Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../base/test/common/utils.js';
import type { IConfigurationChangeEvent } from '../../../platform/configuration/common/configuration.js';
import { SyncDescriptor } from '../../../platform/instantiation/common/descriptors.js';
import { ServiceCollection } from '../../../platform/instantiation/common/serviceCollection.js';
import { LogLevel, NullLogService } from '../../../platform/log/common/log.js';
import { IWorkbenchConfigurationService } from '../../services/configuration/configurationService.js';
import { Workbench } from '../../workbench.js';

/** When the settings arrive, and how often the workbench released them. */
interface StartupSettings {
	readonly loaded: DeferredPromise<void>;
	disposed: number;
}

/** Settings that arrive when `settings.loaded` completes; the workbench creates it. */
class TestStartupConfiguration {
	declare public readonly _serviceBrand: undefined;
	/** The settings never change during startup. */
	public readonly onDidChangeConfiguration: Event<IConfigurationChangeEvent> = () => Disposable.None;

	constructor(private readonly settings: StartupSettings) { }

	public initialize(): Promise<void> {
		return this.settings.loaded.p;
	}

	public dispose(): void {
		this.settings.disposed++;
	}
}

suite('Workbench', () => {
	// First, so the subject built in `setup` is tracked; its store is released before the check.
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let parent: HTMLElement;
	let workbench: Workbench;
	let start: Promise<void>;
	let settings: StartupSettings;
	setup(() => {
		settings = { loaded: new DeferredPromise<void>(), disposed: 0 };
		parent = $('div');
		workbench = disposables.add(new Workbench(parent, { windowId: 1, osRelease: 'test', platform: 'darwin', version: 'test', engineAvailable: false, enableSmokeTestDriver: false, logsPath: '/logs', logLevel: LogLevel.Info }, new ServiceCollection([IWorkbenchConfigurationService, new SyncDescriptor(TestStartupConfiguration, [settings])]), new NullLogService()));
		start = workbench.startup();
		// Disposed while it waits for its settings; they arrive afterwards.
		workbench.dispose();
		void settings.loaded.complete();
	});
	test('a startup ended by disposal fails', async () => {
		await assert.rejects(start);
	});
	test('a startup ended by disposal renders no parts', async () => {
		await start.catch(() => undefined);
		assert.strictEqual(parent.children.length, 0);
	});
	test('a startup ended by disposal releases its services once', async () => {
		await start.catch(() => undefined);
		assert.strictEqual(settings.disposed, 1);
	});
	test('starting again returns the same startup rather than a second lifetime', async () => {
		await start.catch(() => undefined);
		assert.strictEqual(workbench.startup(), start);
	});
});

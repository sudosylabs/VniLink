// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { BrowserWindow } from 'electron';
import { DeferredPromise } from '../../../../base/common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../log/common/log.js';
import { WindowsMainService } from '../../electron-main/windowsMainService.js';
import { upcastPartial } from '../../../../base/test/common/mock.js';

/** A window whose page load the test finishes, counting how often it is shown. */
class TestBrowserWindow implements Partial<BrowserWindow> {
	public readonly id = 1;
	public shown = 0;
	public focused = 0;
	public readonly load = new DeferredPromise<void>();

	/** The service listens for `closed`, which these tests never emit. */
	public readonly once: BrowserWindow['once'] = () => upcastPartial<BrowserWindow>(this);

	public loadURL(): Promise<void> {
		return this.load.p;
	}

	public isDestroyed(): boolean {
		return false;
	}

	public isMinimized(): boolean {
		return false;
	}

	public show(): void {
		this.shown++;
	}

	public focus(): void {
		this.focused++;
	}
}

/** The service with the window it creates replaced, as `BaseWindow` is given its window in VS Code's `windowImpl.test.ts`. */
class TestWindowsMainService extends WindowsMainService {
	public readonly window = new TestBrowserWindow();

	protected override createWindow(): BrowserWindow {
		return upcastPartial<BrowserWindow>(this.window);
	}
}

suite('WindowsMainService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let service: TestWindowsMainService;
	setup(() => {
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(ILogService, disposables.add(new NullLogService()));
		service = disposables.add(instantiationService.createInstance(TestWindowsMainService));
	});
	test('a window asked for while its page loads stays hidden, not blank', async () => {
		const opened = service.openMainWindow(false);
		service.focusMainWindow();
		assert.strictEqual(service.window.shown, 0);
		await service.window.load.complete();
		await opened;
	});
	test('a request made while the page loads shows the window once the page is there', async () => {
		const opened = service.openMainWindow(false);
		service.focusMainWindow();
		await service.window.load.complete();
		await opened;
		assert.strictEqual(service.window.shown, 1);
	});
	test('a loaded window is shown each time it is asked for', async () => {
		const opened = service.openMainWindow(false);
		service.focusMainWindow();
		await service.window.load.complete();
		await opened;
		service.focusMainWindow();
		assert.strictEqual(service.window.shown, 2);
	});
	test('a window opened hidden with no request stays hidden after its page loads', async () => {
		const opened = service.openMainWindow(false);
		await service.window.load.complete();
		await opened;
		assert.strictEqual(service.window.shown, 0);
	});
});

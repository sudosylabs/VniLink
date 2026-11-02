// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { DeferredPromise, timeout } from '../../../base/common/async.js';
import { Emitter } from '../../../base/common/event.js';
import { DisposableStore } from '../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../base/test/common/utils.js';
import type { ShutdownEvent } from '../../../platform/lifecycle/electron-main/lifecycleMainService.js';
import { NullLogService } from '../../../platform/log/common/log.js';
import { joinShutdown, MainWindowRequests, runsInBackground, sharesScreen, startsWithoutWindow } from '../../electron-main/applicationStartup.js';

/** The windows service as requests see it: whether the main window is open, and how often it was focused. */
class TestWindows {
	/** Only the window's presence is read. */
	private readonly window = {};
	public isOpen = false;
	public focused = 0;

	public getMainWindow(): object | undefined {
		return this.isOpen ? this.window : undefined;
	}

	public focusMainWindow(): void {
		this.focused++;
	}
}

class RecordingLogService extends NullLogService {
	public readonly errors: string[] = [];

	public override error(message: string | Error): void {
		this.errors.push(String(message));
	}
}

suite('MainWindowRequests', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let windows: TestWindows;
	let lifecycle: { quitting: boolean };
	let log: RecordingLogService;
	let opened: number;
	/** What the next window open does; by default it opens the window. */
	let open: () => Promise<void>;
	let requests: MainWindowRequests;
	setup(() => {
		windows = new TestWindows();
		lifecycle = { quitting: false };
		log = disposables.add(new RecordingLogService());
		opened = 0;
		open = () => {
			windows.isOpen = true;
			return Promise.resolve();
		};
		requests = new MainWindowRequests(windows, lifecycle, () => {
			opened++;
			return open();
		}, log);
	});
	test('a request while the window is open focuses it rather than opening another', () => {
		requests.openedAtStartup();
		windows.isOpen = true;
		requests.request();
		assert.strictEqual(windows.focused, 1);
		assert.strictEqual(opened, 0);
	});
	test('requests while no window is open open exactly one', async () => {
		// A second launch and an activation arrive together.
		const load = new DeferredPromise<void>();
		open = () => load.p;
		requests.openedAtStartup();
		requests.request();
		requests.request();
		await load.complete();
		assert.strictEqual(opened, 1);
	});
	test('a request after the window closed opens it again', async () => {
		requests.openedAtStartup();
		requests.request();
		await timeout(0);
		windows.isOpen = false;
		requests.request();
		await timeout(0);
		assert.strictEqual(opened, 2);
	});
	test('a request during startup opens no second window', async () => {
		requests.request();
		requests.openedAtStartup();
		await timeout(0);
		assert.strictEqual(opened, 0);
		assert.strictEqual(windows.focused, 0);
	});
	test('a request during a windowless start opens the window once startup is ready', async () => {
		requests.request();
		assert.strictEqual(opened, 0);
		requests.startedWithoutWindow();
		await timeout(0);
		assert.strictEqual(opened, 1);
	});
	test('a request after a windowless start opens the window', () => {
		requests.startedWithoutWindow();
		requests.request();
		assert.strictEqual(opened, 1);
	});
	test('a windowless start nobody asked to see opens no window', () => {
		requests.startedWithoutWindow();
		assert.strictEqual(opened, 0);
	});
	test('requests while quitting open nothing', () => {
		requests.startedWithoutWindow();
		lifecycle.quitting = true;
		requests.request();
		assert.strictEqual(opened, 0);
	});
	test('a window that fails to open is logged, and the next request tries again', async () => {
		open = () => Promise.reject(new Error('The page did not load'));
		requests.openedAtStartup();
		requests.request();
		await timeout(0);
		assert.deepStrictEqual(log.errors, ['Error: The page did not load']);
		requests.request();
		await timeout(0);
		assert.strictEqual(opened, 2);
	});
});

suite('application startup', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const background: readonly (readonly [description: string, restricted: readonly string[], value: unknown, expected: boolean])[] = [
		['runs in the background by default', [], undefined, true],
		['runs in the background when the owner chose it', [], true, true],
		['quits with its last window when the owner chose that', [], false, false],
		['quits with its last window where no tray icon shows', ['application.runInBackground'], true, false],
	];
	for (const [description, restricted, value, expected] of background) {
		test(`the application ${description}`, () => {
			assert.strictEqual(runsInBackground(restricted, value), expected);
		});
	}
	const windowless: readonly (readonly [description: string, temporaryEngine: boolean, inBackground: boolean, atLogin: boolean, expected: boolean])[] = [
		['a login launch that runs in the background starts without a window', false, true, true, true],
		['a launch by the user opens the window', false, true, false, false],
		['a login launch that quits with its window opens the window', false, false, true, false],
		['a temporary engine opens the window', true, true, true, false],
	];
	for (const [description, temporaryEngine, inBackground, atLogin, expected] of windowless) {
		test(description, () => {
			assert.strictEqual(startsWithoutWindow(temporaryEngine, inBackground, () => atLogin), expected);
		});
	}
	test('a temporary engine does not ask how it was launched', () => {
		startsWithoutWindow(true, true, () => assert.fail('the login item was read'));
	});
	test('an installed engine shares the screen', () => {
		assert.strictEqual(sharesScreen({}), true);
	});
	test('a temporary engine that may not capture does not share the screen', () => {
		assert.strictEqual(sharesScreen({ temporaryEngine: true }), false);
	});
	test('a temporary engine that may capture shares the screen', () => {
		assert.strictEqual(sharesScreen({ temporaryEngine: true, temporaryEngineCaptures: true }), true);
	});
	test('quitting waits for the engine to stop and for settings and UI state to be saved', () => {
		const disposables = new DisposableStore();
		const willShutdown = disposables.add(new Emitter<ShutdownEvent>());
		const stopped = Promise.resolve();
		const flushed = Promise.resolve();
		const closed = Promise.resolve();
		const participants = { engine: { stop: () => stopped }, configuration: { flush: () => flushed }, state: { close: () => closed } };
		disposables.add(joinShutdown(willShutdown.event, participants));
		const joined: [string, Promise<unknown>][] = [];
		willShutdown.fire({ join: (id, promise) => joined.push([id, promise]) });
		disposables.dispose();
		const waitsFor = joined.map(([id, promise]) => [id, promise === stopped ? 'stop' : promise === flushed ? 'flush' : promise === closed ? 'close' : 'other']);
		assert.deepStrictEqual(waitsFor, [['engine', 'stop'], ['configuration', 'flush'], ['state', 'close']]);
	});
});

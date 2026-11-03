// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { EventEmitter } from 'events';
import { DeferredPromise, timeout } from '../../../../base/common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../base/test/common/virtualScheduling/runWithFakedTimers.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../log/common/log.js';
import { LifecycleMainService, type LifecycleApplication } from '../../electron-main/lifecycleMainService.js';

class TestQuitEvent {
	public defaultPrevented = false;

	public preventDefault(): void {
		this.defaultPrevented = true;
	}
}

/**
 * Electron's `app` as quitting sees it: `quit` emits `before-quit`, and quits
 * (emitting `will-quit`) unless a listener prevents it.
 */
class TestApplication extends EventEmitter implements LifecycleApplication {
	public quits = 0;
	public quitCompleted = false;
	public relaunches = 0;

	public quit(): void {
		this.quits++;
		const event = new TestQuitEvent();
		this.emit('before-quit', event);
		if (!event.defaultPrevented) {
			this.quitCompleted = true;
			this.emit('will-quit');
		}
	}

	public relaunch(): void {
		this.relaunches++;
	}

	public exit(): void {
		throw new Error('Quitting exits only in the development runner');
	}
}

class RecordingLogService extends NullLogService {
	public readonly errors: string[] = [];

	public override error(message: string | Error): void {
		this.errors.push(String(message));
	}
}

suite('LifecycleMainService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let application: TestApplication;
	let log: RecordingLogService;
	let lifecycle: LifecycleMainService;
	setup(() => {
		application = new TestApplication();
		log = disposables.add(new RecordingLogService());
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(ILogService, log);
		lifecycle = disposables.add(instantiationService.createInstance(LifecycleMainService, application, new EventEmitter()));
	});

	/** Joins a participant that finishes when the returned promise is completed, counting how often it is asked. */
	function joinParticipant(): { readonly finish: DeferredPromise<void>; readonly joins: () => number } {
		const finish = new DeferredPromise<void>();
		let joins = 0;
		disposables.add(lifecycle.onWillShutdown(event => {
			joins++;
			event.join('engine', finish.p);
		}));
		return { finish, joins: () => joins };
	}
	test('quitting waits for a shutdown participant', async () => {
		const participant = joinParticipant();
		application.quit();
		assert.strictEqual(application.quitCompleted, false);
		await participant.finish.complete();
		await timeout(0);
		assert.strictEqual(application.quitCompleted, true);
	});
	test('repeated quit requests ask each participant once', async () => {
		const participant = joinParticipant();
		application.quit();
		application.quit();
		assert.strictEqual(participant.joins(), 1);
		await participant.finish.complete();
		await timeout(0);
		assert.strictEqual(participant.joins(), 1);
	});
	test('once its participants finish, the application quits once more and completes', async () => {
		const participant = joinParticipant();
		application.quit();
		application.quit();
		await participant.finish.complete();
		await timeout(0);
		assert.strictEqual(application.quits, 3);
	});
	test('relaunching twice restarts a built application once', async () => {
		lifecycle.relaunch();
		lifecycle.relaunch();
		await timeout(0);
		assert.strictEqual(application.relaunches, 1);
	});
	test('relaunching quits through shutdown', async () => {
		const participant = joinParticipant();
		lifecycle.relaunch();
		assert.strictEqual(participant.joins(), 1);
		await participant.finish.complete();
		await timeout(0);
		assert.strictEqual(application.quitCompleted, true);
	});
	test('quitting gives a slow participant until the deadline', () => runWithFakedTimers({}, async () => {
		joinParticipant();
		application.quit();
		await timeout(19_999);
		assert.strictEqual(application.quits, 1);
	}));
	test('a hung participant does not keep the process alive past the deadline', () => runWithFakedTimers({}, async () => {
		joinParticipant();
		application.quit();
		await timeout(20_000);
		await timeout(0);
		assert.strictEqual(application.quits, 2);
		assert.strictEqual(application.quitCompleted, true);
	}));
	test('quitting past the deadline names only the participants it left behind', () => runWithFakedTimers({}, async () => {
		disposables.add(lifecycle.onWillShutdown(event => {
			event.join('engine', new DeferredPromise<void>().p);
			event.join('configuration', Promise.resolve());
		}));
		application.quit();
		await timeout(20_000);
		await timeout(0);
		assert.deepStrictEqual(log.errors, ['Quitting without waiting longer for engine, still running after 20 seconds']);
	}));
	test('a failed participant is logged and does not stop quitting', async () => {
		disposables.add(lifecycle.onWillShutdown(event => {
			event.join('state', Promise.reject(new Error('disk full')));
		}));
		application.quit();
		await timeout(0);
		assert.deepStrictEqual(log.errors, ['Shutdown participant state failed']);
		assert.strictEqual(application.quitCompleted, true);
	});
	test('Windows ending the session runs the shutdown participants', async () => {
		const participant = joinParticipant();
		application.emit('session-end');
		assert.strictEqual(participant.joins(), 1);
		await participant.finish.complete();
	});
	test('Windows ending the session marks the application as quitting', () => {
		application.emit('session-end');
		assert.strictEqual(lifecycle.quitting, true);
	});
});

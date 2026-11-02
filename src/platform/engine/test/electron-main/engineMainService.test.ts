// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../base/common/async.js';
import { Emitter, type Event } from '../../../../base/common/event.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import type { IChannel } from '../../../../base/parts/ipc/common/ipc.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../base/test/common/virtualScheduling/index.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../log/common/log.js';
import { EngineMainService, type IEngineConnection, type IEngineProcess } from '../../electron-main/engineMainService.js';

/** Answers the engine host gives, one per command, in order; a command with none is answered `done`. */
type Answer = () => Promise<unknown>;

/** The engine host's IPC channel, answering each command from `answers`. */
class TestEngineHostChannel implements IChannel {
	constructor(private readonly answers: Answer[]) { }

	public call<T>(): Promise<T> {
		return (this.answers.shift()?.() ?? Promise.resolve({ type: 'done' })) as Promise<T>;
	}

	public listen<T>(): Event<T> {
		// The engine double sends no events.
		return () => Disposable.None;
	}
}

/** An engine process the test starts, answers and crashes. */
class TestEngineProcess extends Disposable implements IEngineProcess {
	private readonly stderr = this._register(new Emitter<Uint8Array>());
	public readonly onStderr = this.stderr.event;
	private readonly message = this._register(new Emitter<unknown>());
	public readonly onMessage = this.message.event;
	private readonly exit = this._register(new Emitter<number>());
	public readonly onExit = this.exit.event;
	public killed = false;

	constructor(private readonly answers: Answer[]) {
		super();
	}

	public connect(): IEngineConnection {
		return { channel: new TestEngineHostChannel(this.answers), dispose: () => { /* Nothing holds the channel open. */ } };
	}

	public postMessage(): void {
		// The engine double takes no data channels.
	}

	public kill(): void {
		this.killed = true;
	}

	/** The engine opened its installation and accepts commands. */
	public ready(): void {
		this.message.fire({ ready: true });
	}

	/** The engine could not open its installation. */
	public refuse(error: string): void {
		this.message.fire({ ready: false, error });
	}

	public crash(code: number): void {
		this.exit.fire(code);
	}

	public print(text: string): void {
		this.stderr.fire(new TextEncoder().encode(text));
	}
}

/** A log that keeps the messages of its warnings and errors. */
class TestLogService extends NullLogService {
	public readonly warnings: string[] = [];
	public readonly errors: string[] = [];

	public override warn(message: string): void {
		this.warnings.push(message);
	}

	public override error(message: string | Error): void {
		this.errors.push(String(message));
	}
}

suite('EngineMainService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	/** Every engine process started, oldest first. */
	let forks: TestEngineProcess[];
	let answers: Answer[];
	let log: TestLogService;
	/** What windows would be told, and when main re-applied its defaults. */
	let story: string[];
	function createService(): EngineMainService {
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(ILogService, log);
		const service = disposables.add(new EngineMainService(instantiationService.get(ILogService), () => {
			const process = disposables.add(new TestEngineProcess(answers));
			forks.push(process);
			return process;
		}));
		disposables.add(service.onDidChangeAvailability(available => story.push(available ? 'available' : 'unavailable')));
		disposables.add(service.onDidChangeRestarting(restarting => story.push(restarting ? 'restarting' : 'steady')));
		// Main re-applies defaults, settings and permissions here; services skip that work while the engine reads unavailable.
		service.setRestartHandler(() => {
			story.push(service.available ? 'defaults applied' : 'defaults skipped: engine reads unavailable');
			return Promise.resolve();
		});
		return service;
	}

	/** Starts a service and brings its first engine up; the story starts after that. */
	async function startService(): Promise<EngineMainService> {
		const service = createService();
		const started = service.start();
		newest().ready();
		await started;
		story.length = 0;
		return service;
	}
	function newest(): TestEngineProcess {
		const process = forks.at(-1);
		if (!process) {
			throw new Error('No engine was started');
		}
		return process;
	}

	/** Crashes the newest engine, checks the next is started `delay` later and not sooner, and brings it up. */
	async function crashAndRecover(delay: number): Promise<void> {
		newest().crash(1);
		const before = forks.length;
		await timeout(delay - 1);
		assert.strictEqual(forks.length, before, `The engine restarted before ${String(delay)} ms`);
		await timeout(1);
		assert.strictEqual(forks.length, before + 1, `The engine did not restart after ${String(delay)} ms`);
		newest().ready();
		await timeout(0);
	}
	setup(() => {
		forks = [];
		answers = [];
		log = new TestLogService();
		story = [];
	});
	test('windows learn that a crashed engine is coming back before they learn it is gone', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		newest().crash(134);
		assert.deepStrictEqual(story, ['restarting', 'unavailable']);
		assert.strictEqual(service.available, false);
		assert.strictEqual(service.isRestarting, true);
		await service.stop();
	}));
	test('commands fail while a crashed engine waits to restart', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		newest().crash(134);
		await assert.rejects(service.execute({ type: 'snapshot' }), /Engine unavailable/);
		await service.stop();
	}));
	test('a crashed engine is started again a second later, and its defaults are applied before windows hear it is back', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		await crashAndRecover(1000);
		assert.deepStrictEqual(story, ['restarting', 'unavailable', 'defaults applied', 'steady', 'available']);
		assert.strictEqual(service.available, true);
		await service.stop();
	}));
	test('restarts wait twice as long each time, up to sixteen seconds', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		for (const delay of [1000, 2000, 4000, 8000, 16000]) {
			await crashAndRecover(delay);
		}
		await service.stop();
	}));
	test('after five restarts in a row the engine is given up, and windows are told it is gone for good', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		for (const delay of [1000, 2000, 4000, 8000, 16000]) {
			await crashAndRecover(delay);
		}
		story.length = 0;
		newest().crash(1);
		await timeout(60 * 60 * 1000);
		assert.strictEqual(forks.length, 6, 'The engine was started a sixth time');
		assert.strictEqual(service.isRestarting, false);
		assert.strictEqual(service.available, false);
		assert.deepStrictEqual(story, ['unavailable']);
	}));
	test('an engine that stays up for a minute starts its attempts over', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		await crashAndRecover(1000);
		await timeout(60_000);
		await crashAndRecover(1000);
		await service.stop();
	}));
	test('a restarted engine that cannot open its installation is tried again', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		newest().crash(1);
		await timeout(1000);
		newest().refuse('StoreLocked');
		await timeout(0);
		assert.strictEqual(service.isRestarting, true);
		await timeout(2000);
		assert.strictEqual(forks.length, 3);
		await service.stop();
	}));
	test('stopping the service ends the engine without restarting it', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		const stopped = service.stop();
		newest().crash(0);
		await stopped.catch(() => { /* The engine exited before it answered the shutdown. */ });
		await timeout(60_000);
		assert.strictEqual(forks.length, 1);
		assert.strictEqual(service.isRestarting, false);
	}));
	test('a command that does not answer in time fails and says its outcome is unknown', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		answers.push(() => new Promise(() => { /* The engine never answers. */ }));
		const slow = assert.rejects(service.execute({ type: 'startHosting' }), /timed out and may still finish/);
		await timeout(10_000);
		await slow;
		assert.deepStrictEqual(log.warnings, ['[engine] startHosting did not answer in time; its outcome is unknown.']);
		await service.stop();
	}));
	test('a command that timed out does not hold the next one', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		answers.push(() => new Promise(() => { /* The engine never answers. */ }));
		const slow = assert.rejects(service.execute({ type: 'startHosting' }));
		await timeout(10_000);
		await slow;
		assert.deepStrictEqual(await service.execute({ type: 'snapshot' }), { type: 'done' });
		await service.stop();
	}));
	test('a command that timed out does not restart the engine', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		answers.push(() => new Promise(() => { /* The engine never answers. */ }));
		const slow = assert.rejects(service.execute({ type: 'startHosting' }));
		await timeout(10_000);
		await slow;
		assert.strictEqual(service.available, true);
		assert.strictEqual(forks.length, 1);
		await service.stop();
	}));
	test('an engine that fails its first start is tried again, and the restart applies its defaults', () => runWithFakedTimers({}, async () => {
		const service = createService();
		const started = assert.rejects(service.start(), /StoreLocked/);
		newest().refuse('StoreLocked');
		await started;
		assert.strictEqual(service.isRestarting, true);
		await timeout(1000);
		assert.strictEqual(forks.length, 2);
		newest().ready();
		await timeout(0);
		assert.deepStrictEqual(story, ['restarting', 'defaults applied', 'steady', 'available']);
		assert.strictEqual(service.available, true);
		await service.stop();
	}));
	test('stopping while the engine starts fails the start at once, not at its timeout', () => runWithFakedTimers({}, async () => {
		const service = createService();
		let outcome = 'pending';
		service.start().then(() => {
			outcome = 'started';
		}, () => {
			outcome = 'failed';
		});
		await service.stop();
		await timeout(0);
		assert.strictEqual(outcome, 'failed');
		assert.strictEqual(forks[0]?.killed, true);
	}));
	test('what the engine prints on standard error reaches the main log one whole line at a time', () => runWithFakedTimers({}, async () => {
		const service = createService();
		const started = service.start();
		newest().print('thread main panicked at ');
		newest().print('src/lib.rs:1\nnote: run with RUST_BACKTRACE\n\npartial');
		newest().ready();
		await started;
		assert.deepStrictEqual(log.errors, ['[engine] thread main panicked at src/lib.rs:1', '[engine] note: run with RUST_BACKTRACE']);
		await service.stop();
	}));
	test('the engine\'s last line on standard error, without its newline, is logged when it exits', () => runWithFakedTimers({}, async () => {
		const service = await startService();
		newest().print('partial');
		newest().crash(101);
		assert.ok(log.errors.includes('[engine] partial'));
		await service.stop();
	}));
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../../base/common/async.js';
import { DisposableStore } from '../../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import type { DataPacket } from '../../../../../platform/engine/common/dataChannel.js';
import type { RemoteStatus, SessionInfo } from '../../../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../../../platform/engine/common/engineService.js';
import { TestEngineService } from '../../../../../platform/engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import type { IProcessDataEvent, TerminalExit, TerminalLaunchFailure } from '../../../../../platform/terminal/common/terminal.js';
import { IDeviceService } from '../../../devices/deviceService.js';
import { EnginePty, type TerminalChannelOpener } from '../../enginePty.js';

const deviceId = 'a'.repeat(64);

const session: SessionInfo = { id: '7', generation: '3', role: 'Controller', peer_id: deviceId, phase: 'Active' };

interface ShellOptions {
	/** What the computer reports for its terminal; `null` is a computer that reports nothing. */
	readonly status?: RemoteStatus | null;
	readonly sessions?: SessionInfo[];
	readonly openFails?: boolean;
	/** Writes stay in flight until a test releases them from `pendingWrites`. */
	readonly writeDelay?: boolean;
}

/** Output an event carries, as text. */
function text(event: IProcessDataEvent): string {
	return typeof event.data === 'string' ? event.data : new TextDecoder().decode(event.data);
}

suite('EnginePty', () => {
	const disposables = new DisposableStore();

	/** A shell on `deviceId` at 100×30, with its channel, its engine and what it fired recorded. */
	function shell({ status = 'available', sessions = [session], openFails = false, writeDelay = false }: ShellOptions = {}) {
		const writes: string[] = [];
		const bytes: number[][] = [];
		const resizes: [number, number][] = [];
		const requests: Parameters<TerminalChannelOpener>[0][] = [];
		const pendingWrites: (() => void)[] = [];
		let consume: (packet: DataPacket) => Promise<void> = () => Promise.reject(new Error('The channel is not open'));
		let onError: (error: Error) => void = () => {
			throw new Error('The channel is not open');
		};
		let closed = 0;
		const opener: TerminalChannelOpener = (request, consumer, errorHandler) => {
			requests.push(request);
			if (openFails) {
				return Promise.reject(new Error('Data channel unavailable'));
			}
			consume = consumer;
			onError = errorHandler;
			return Promise.resolve({
				write: data => {
					writes.push(new TextDecoder().decode(data));
					bytes.push([...data]);
					return writeDelay ? new Promise<void>(resolve => pendingWrites.push(resolve)) : Promise.resolve();
				},
				resize: (rows, columns) => resizes.push([rows, columns]),
				close: () => closed++,
			});
		};
		const engine = disposables.add(new TestEngineService(command => {
			assert.strictEqual(command.type, 'controllerSessions');
			return { type: 'sessions', sessions };
		}));
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IEngineService, engine);
		instantiationService.stub(IDeviceService, {
			availability: (id: string) => {
				assert.strictEqual(id, deviceId);
				return Promise.resolve(status === null ? null : [{ capability: 'terminal', status }]);
			},
		});
		const pty = disposables.add(instantiationService.createInstance(EnginePty, deviceId, 100, 30, opener));
		const data: IProcessDataEvent[] = [];
		const exits: TerminalExit[] = [];
		let ready = 0;
		disposables.add(pty.onProcessData(event => data.push(event)));
		disposables.add(pty.onProcessExit(exit => exits.push(exit)));
		disposables.add(pty.onProcessReady(() => ready++));
		return {
			pty, writes, bytes, resizes, requests, data, exits, pendingWrites,
			get ready() {
				return ready;
			},
			get closed() {
				return closed;
			},
			packet: (kind: string, payload?: string, metadata = '{}') => consume({ type: 'data', sequence: 1, kind, data: payload === undefined ? undefined : new TextEncoder().encode(payload), metadata }),
			lose: () => {
				onError(new Error('Data channel disconnected'));
			},
		};
	}
	type Shell = ReturnType<typeof shell>;
	teardown(() => {
		disposables.clear();
	});
	test('a shell opens on the active controller session at the view size and reports ready', async () => {
		const f = shell();
		assert.strictEqual(await f.pty.start(), undefined);
		assert.deepStrictEqual(f.requests, [{ kind: 'terminal', sessionId: '7', generation: '3', rows: 30, columns: 100 }]);
		assert.strictEqual(f.ready, 1);
	});
	test('a started shell cannot be started again', async () => {
		const f = shell();
		await f.pty.start();
		assert.deepStrictEqual(await f.pty.start(), { code: 'failed' });
	});
	test('output is acknowledged only after the emulator parsed it', async () => {
		const f = shell();
		await f.pty.start();
		let settled = false;
		const first = f.packet('terminal', 'hello').then(() => {
			settled = true;
		});
		await timeout(0);
		assert.strictEqual(settled, false, 'output was acknowledged before the emulator parsed it');
		assert.strictEqual(text(f.data[0]), 'hello');
		f.data[0].acknowledge();
		await first;
		assert.strictEqual(settled, true);
	});
	test('a gap in the output is shown as a message the emulator acknowledges', async () => {
		const f = shell();
		await f.pty.start();
		const gap = f.packet('gap', undefined, '{"lostBytes":"42"}');
		assert.match(text(f.data[0]), /Some output was skipped/);
		f.data[0].acknowledge();
		await gap;
	});
	const endings: [string, (f: Shell) => unknown, TerminalExit][] = [
		['an exit', f => f.packet('exited', undefined, '{"code":3}'), { code: 3 }],
		['a failure', f => f.packet('failed'), { reason: 'failed' }],
		['a lost channel', f => {
			f.lose();
		}, { reason: 'lost' }],
	];
	for (const [name, end, expected] of endings) {
		test(`${name} ends the shell once and closes the channel`, async () => {
			const f = shell();
			await f.pty.start();
			await end(f);
			assert.deepStrictEqual(f.exits, [expected]);
			assert.strictEqual(f.closed, 1);
			f.lose();
			f.pty.input('late');
			assert.deepStrictEqual(f.exits, [expected], 'a late event ended the shell twice');
			assert.deepStrictEqual(f.writes, [], 'input reached an ended shell');
		});
	}
	const malformed: [string, string, string | undefined, string][] = [
		['an exit with a negative code', 'exited', undefined, '{"code":-1}'],
		['an exit that is not JSON', 'exited', undefined, 'not json'],
		['output without data', 'terminal', undefined, '{}'],
		['a packet of another kind', 'video', 'x', '{}'],
	];
	for (const [name, kind, payload, metadata] of malformed) {
		test(`${name} is refused instead of reaching the emulator`, async () => {
			const f = shell();
			await f.pty.start();
			await assert.rejects(f.packet(kind, payload, metadata));
			assert.deepStrictEqual(f.data, []);
		});
	}
	test('input typed before the shell is ready is sent once it is', async () => {
		const f = shell();
		f.pty.input('typed before ready');
		await f.pty.start();
		assert.deepStrictEqual(f.writes, ['typed before ready']);
	});
	test('input is UTF-8, split into channel-sized writes, and waits for writes in flight', async () => {
		const f = shell({ writeDelay: true });
		await f.pty.start();
		f.pty.input('é✓');
		f.pty.input('x'.repeat(16384 * 9));
		assert.strictEqual(f.writes.length, 8, 'more writes went out than the channel holds');
		assert.strictEqual(f.writes[0], 'é✓');
		for (let release = f.pendingWrites.shift(); release; release = f.pendingWrites.shift()) {
			release();
			await timeout(0);
		}
		assert.strictEqual(f.writes.length, 10);
		assert.ok(f.writes.slice(1).every(write => write.length <= 16384));
		assert.strictEqual(f.writes.slice(1).join('').length, 16384 * 9);
	});
	test('binary input is sent one byte per character, not UTF-8 encoded', async () => {
		const f = shell();
		await f.pty.start();
		f.pty.processBinary('\x1b[M\xff');
		assert.deepStrictEqual(f.bytes.at(-1), [0x1b, 0x5b, 0x4d, 0xff]);
	});
	test('input beyond what the shell can take is refused with a message, not held', async () => {
		const f = shell({ writeDelay: true });
		await f.pty.start();
		f.pty.input('y'.repeat(1024 * 1024 + 1));
		assert.strictEqual(f.writes.length, 0);
		assert.match(text(f.data[0]), /wasn't sent/);
	});
	test('a resize before the shell starts opens it at the new size', async () => {
		const f = shell();
		f.pty.resize(120, 40);
		await f.pty.start();
		assert.strictEqual(f.requests[0].rows, 40);
		assert.strictEqual(f.requests[0].columns, 120);
	});
	test('resize clamps to the host grid and skips repeats', async () => {
		const f = shell();
		f.pty.resize(120, 40);
		await f.pty.start();
		f.pty.resize(120, 40);
		f.pty.resize(5000, 0);
		f.pty.resize(Number.NaN, 2.7);
		assert.deepStrictEqual(f.resizes, [[1, 1024], [2, 1]]);
	});
	const refusals: [string, ShellOptions, TerminalLaunchFailure][] = [
		['a computer that reports nothing', { status: null }, 'disconnected'],
		['a terminal turned off', { status: 'turnedOff' }, 'notAllowed'],
		['a terminal that needs permission', { status: 'needsPermission' }, 'notAllowed'],
		['an unsupported terminal', { status: 'unsupported' }, 'unsupported'],
		['an unavailable terminal', { status: 'unavailable' }, 'unavailable'],
		['no controller session', { sessions: [] }, 'disconnected'],
		['a session still connecting', { sessions: [{ ...session, phase: 'Connecting' }] }, 'disconnected'],
	];
	for (const [name, options, code] of refusals) {
		test(`${name} refuses the launch as ${code} before any channel opens`, async () => {
			const f = shell(options);
			assert.deepStrictEqual(await f.pty.start(), { code });
			assert.strictEqual(f.ready, 0);
			assert.strictEqual(f.requests.length, 0);
		});
	}
	test('a channel that cannot open refuses the launch as failed', async () => {
		const f = shell({ openFails: true });
		assert.deepStrictEqual(await f.pty.start(), { code: 'failed' });
		assert.strictEqual(f.ready, 0);
		assert.strictEqual(f.requests.length, 1);
	});
	test('disposing while starting closes the late channel and fires nothing', async () => {
		const f = shell();
		const starting = f.pty.start();
		f.pty.dispose();
		assert.strictEqual(await starting, undefined);
		assert.strictEqual(f.ready, 0);
		assert.deepStrictEqual(f.exits, []);
		assert.ok(f.requests.length === 0 || f.closed === 1, 'a channel opened after disposal stayed open');
	});
	test('disposing releases output still waiting for the emulator and closes the channel once', async () => {
		const f = shell();
		await f.pty.start();
		const pending = f.packet('terminal', 'never parsed');
		f.pty.dispose();
		await pending;
		assert.deepStrictEqual(f.exits, [], 'disposal reported an exit');
		assert.strictEqual(f.closed, 1);
		f.pty.dispose();
		assert.strictEqual(f.closed, 1, 'disposal closed the channel twice');
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const output = await build({
	stdin: { contents: `export { EnginePty } from './src/workbench/services/terminal/enginePty.js';`, resolveDir: process.cwd(), loader: 'ts' },
	bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json',
});

const { EnginePty } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

const deviceId = 'a'.repeat(64);

const session = { id: '7', generation: '3', role: 'Controller', peer_id: deviceId, phase: 'Active' };

function fixture({ status = 'available', report = true, sessions = [session], openFails = false, writeDelay = false } = {}) {
	const writes = [];
	const bytes = [];
	const resizes = [];
	const requests = [];
	let consume;
	let onError;
	let closed = 0;
	const pendingWrites = [];
	const channel = {
		write: data => {
			writes.push(Buffer.from(data).toString());
			bytes.push([...data]);
			return writeDelay ? new Promise(resolve => pendingWrites.push(resolve)) : Promise.resolve();
		},
		resize: (rows, columns) => resizes.push([rows, columns]),
		close: () => closed++,
	};
	const engine = { available: true, execute: async command => (assert.equal(command.type, 'controllerSessions'), { type: 'sessions', sessions }) };
	const devices = { availability: async id => (assert.equal(id, deviceId), report ? [{ capability: 'terminal', status }] : null) };
	const opener = async (request, consumer, errorHandler) => {
		requests.push(request);
		if (openFails) {
			throw new Error('Data channel unavailable');
		}
		consume = consumer;
		onError = errorHandler;
		return channel;
	};
	const pty = new EnginePty(deviceId, 100, 30, opener, engine, devices);
	const data = [];
	const exits = [];
	let ready = 0;
	pty.onProcessData(event => data.push(event));
	pty.onProcessExit(exit => exits.push(exit));
	pty.onProcessReady(() => ready++);
	return {
		pty, writes, bytes, resizes, requests, data, exits, pendingWrites,
		get ready() {
			return ready;
		},
		get closed() {
			return closed;
		},
		packet: (kind, payload, metadata = '{}') => consume({ type: 'data', sequence: 1, kind, data: payload === undefined ? undefined : new TextEncoder().encode(payload), metadata }),
		lose: () => onError(new Error('Data channel disconnected')),
	};
}

test('a shell opens on the active controller session at the view size and reports ready', async () => {
	const f = fixture();
	assert.equal(await f.pty.start(), undefined);
	assert.deepEqual(f.requests, [{ kind: 'terminal', sessionId: '7', generation: '3', rows: 30, columns: 100 }]);
	assert.equal(f.ready, 1);
	assert.deepEqual(await f.pty.start(), { code: 'failed' }, 'A started shell started again');
	f.pty.dispose();
});

test('output is acknowledged only after the emulator parsed it, in order', async () => {
	const f = fixture();
	await f.pty.start();
	let settled = false;
	const first = f.packet('terminal', 'hello').then(() => {
		settled = true;
	});
	await new Promise(resolve => setImmediate(resolve));
	assert.equal(settled, false, 'Output was acknowledged before the emulator parsed it');
	assert.equal(new TextDecoder().decode(f.data[0].data), 'hello');
	f.data[0].acknowledge();
	await first;
	assert.equal(settled, true);
	const gap = f.packet('gap', undefined, '{"lostBytes":"42"}');
	assert.match(f.data[1].data, /Some output was skipped/);
	f.data[1].acknowledge();
	await gap;
	f.pty.dispose();
});

test('exit, failure and a lost channel end the shell once and close the channel', async () => {
	for (const [end, expected] of [[f => f.packet('exited', undefined, '{"code":3}'), { code: 3 }], [f => f.packet('failed'), { reason: 'failed' }], [f => f.lose(), { reason: 'lost' }]]) {
		const f = fixture();
		await f.pty.start();
		await end(f);
		assert.deepEqual(f.exits, [expected]);
		assert.equal(f.closed, 1);
		f.lose();
		f.pty.input('late');
		assert.deepEqual(f.exits, [expected], 'A late event ended the shell twice');
		assert.deepEqual(f.writes, [], 'Input reached an ended shell');
		f.pty.dispose();
	}
});

test('malformed packets end the shell instead of reaching the emulator', async () => {
	for (const [kind, payload, metadata] of [['exited', undefined, '{"code":-1}'], ['exited', undefined, 'not json'], ['terminal', undefined, '{}'], ['video', 'x', '{}']]) {
		const f = fixture();
		await f.pty.start();
		await assert.rejects(f.packet(kind, payload, metadata));
		assert.deepEqual(f.data, []);
		f.pty.dispose();
	}
});

test('input is UTF-8, split into channel-sized writes, and waits for writes in flight', async () => {
	const f = fixture({ writeDelay: true });
	f.pty.input('typed before ready');
	await f.pty.start();
	assert.deepEqual(f.writes, ['typed before ready']);
	f.pty.input('é✓');
	f.pty.input('x'.repeat(16384 * 9));
	assert.equal(f.writes.length, 8, 'More writes went out than the channel holds');
	assert.equal(f.writes[1], 'é✓');
	while (f.pendingWrites.length) {
		f.pendingWrites.shift()();
		await new Promise(resolve => setImmediate(resolve));
	}
	assert.equal(f.writes.length, 11);
	assert(f.writes.slice(2).every(write => write.length <= 16384));
	assert.equal(f.writes.slice(2).join('').length, 16384 * 9);
	f.pty.processBinary('\x1b[M\xff');
	assert.deepEqual(f.bytes.at(-1), [0x1b, 0x5b, 0x4d, 0xff], 'Binary input was UTF-8 encoded');
	f.pty.dispose();
});

test('input beyond what the shell can take is refused with a message, not held', async () => {
	const f = fixture({ writeDelay: true });
	await f.pty.start();
	f.pty.input('y'.repeat(1024 * 1024 + 1));
	assert.equal(f.writes.length, 0);
	assert.match(f.data[0].data, /wasn't sent/);
	f.pty.dispose();
});

test('resize clamps to the host grid and skips repeats', async () => {
	const f = fixture();
	f.pty.resize(120, 40);
	await f.pty.start();
	assert.deepEqual(f.requests[0].rows, 40);
	f.pty.resize(120, 40);
	f.pty.resize(5000, 0);
	f.pty.resize(Number.NaN, 2.7);
	assert.deepEqual(f.resizes, [[1, 1024], [2, 1]]);
	f.pty.dispose();
});

test('launch refusals come from what the computer reports, before any channel opens', async () => {
	for (const [options, code] of [[{ report: false }, 'disconnected'], [{ status: 'turnedOff' }, 'notAllowed'], [{ status: 'needsPermission' }, 'notAllowed'], [{ status: 'unsupported' }, 'unsupported'], [{ status: 'unavailable' }, 'unavailable'], [{ sessions: [] }, 'disconnected'], [{ sessions: [{ ...session, phase: 'Connecting' }] }, 'disconnected'], [{ openFails: true }, 'failed']]) {
		const f = fixture(options);
		assert.deepEqual(await f.pty.start(), { code });
		assert.equal(f.ready, 0);
		assert.equal(f.requests.length, options.openFails ? 1 : 0);
		f.pty.dispose();
	}
});

test('shutting down while starting closes the late channel and fires nothing', async () => {
	const f = fixture();
	const starting = f.pty.start();
	f.pty.shutdown();
	assert.equal(await starting, undefined);
	assert.equal(f.ready, 0);
	assert.deepEqual(f.exits, []);
	assert(f.requests.length === 0 || f.closed === 1, 'A channel opened after shutdown stayed open');
	f.pty.dispose();
});

test('shutdown releases output still waiting for the emulator', async () => {
	const f = fixture();
	await f.pty.start();
	const pending = f.packet('terminal', 'never parsed');
	f.pty.shutdown();
	await pending;
	assert.deepEqual(f.exits, [], 'Shutdown reported an exit');
	assert.equal(f.closed, 1);
	f.pty.shutdown();
	assert.equal(f.closed, 1, 'Shutdown closed the channel twice');
	f.pty.dispose();
});

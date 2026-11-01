// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const root = process.cwd().replaceAll('\\', '/');

/** Electron as the supervisor sees it: forked engines the test starts, answers and crashes. */
const electron = `
import { EventEmitter } from 'node:events';
export const forks = [];
export const utilityProcess = { fork(_path, args) {
	const child = Object.assign(new EventEmitter(), { args, killed: false, stdout: { resume() { } }, stderr: new EventEmitter(), postMessage() { }, kill() { child.killed = true; } });
	forks.push(child);
	return child;
} };
export class MessageChannelMain { port1 = {}; port2 = {}; }`;

const ipc = `
import { Event } from '${root}/src/base/common/event.js';
export class Client { getChannel() { return { listen: () => Event.None, call: () => globalThis.engineCalls?.shift()?.() ?? Promise.resolve({ type: 'done' }) }; } dispose() { } }`;

const output = await build({
	stdin: { contents: `export { EngineMainService } from './src/platform/engine/electron-main/engineMainService.ts'; export { forks } from 'electron';`, loader: 'ts', resolveDir: root },
	bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json', banner: { js: "const __dirname = '/engine';" }, plugins: [{
		name: 'electron-fixtures', setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
			builder.onResolve({ filter: /electron-main\/ipc\.mp\.js$/ }, () => ({ path: 'ipc', namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: path === 'electron' ? electron : ipc, loader: 'js', resolveDir: root }));
		}
	}]
});

const { EngineMainService, forks } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

const warnings = [];

const settle = async () => {
	for (let step = 0; step < 20; step++) {
		await Promise.resolve();
	}
};

/** Starts a service, records what windows would be told, and brings the first engine up. */
async function startService(context) {
	context.mock.timers.enable({ apis: ['setTimeout'] });
	forks.length = 0;
	const service = new EngineMainService({ error() { }, warn: message => warnings.push(message) });
	const story = [];
	service.onDidChangeAvailability(available => story.push(available ? 'available' : 'unavailable'));
	service.onDidChangeRestarting(restarting => story.push(restarting ? 'restarting' : 'steady'));
	service.setRestartHandler(async () => {
		story.push('defaults applied');
	});
	const started = service.start();
	forks[0].emit('message', { ready: true });
	await started;
	story.length = 0;
	return { service, story };
}

/** Crashes the newest engine and waits `delay` for the next to be forked, which then reports ready. */
async function crashAndRecover(context, delay) {
	const crashed = forks.at(-1);
	crashed.emit('exit', 1);
	await settle();
	const before = forks.length;
	context.mock.timers.tick(delay - 1);
	await settle();
	assert.equal(forks.length, before, `The engine restarted before ${String(delay)} ms`);
	context.mock.timers.tick(1);
	assert.equal(forks.length, before + 1);
	forks.at(-1).emit('message', { ready: true });
	await settle();
}

test('an engine that crashes is restarted, its defaults are applied again, and windows hear it in that order', async context => {
	const { service, story } = await startService(context);
	assert.equal(service.available, true);
	forks[0].emit('exit', 134);
	await settle();
	assert.deepEqual(story, ['restarting', 'unavailable'], 'Windows must learn that the engine is coming back before they learn it is gone');
	assert.equal(service.available, false);
	assert.equal(service.isRestarting, true);
	await assert.rejects(service.execute({ type: 'snapshot' }), /Engine unavailable/);
	context.mock.timers.tick(1000);
	assert.equal(forks.length, 2);
	forks[1].emit('message', { ready: true });
	await settle();
	assert.deepEqual(story, ['restarting', 'unavailable', 'defaults applied', 'steady', 'available']);
	assert.equal(service.available, true);
	await service.stop();
});

test('restarts wait twice as long each time and are given up after five in a row', async context => {
	const { service, story } = await startService(context);
	for (const delay of [1000, 2000, 4000, 8000, 16000]) {
		await crashAndRecover(context, delay);
	}
	assert.equal(forks.length, 6);
	story.length = 0;
	forks.at(-1).emit('exit', 1);
	await settle();
	context.mock.timers.tick(60 * 60 * 1000);
	await settle();
	assert.equal(forks.length, 6, 'The engine was started a sixth time');
	assert.equal(service.isRestarting, false);
	assert.equal(service.available, false);
	assert.deepEqual(story, ['unavailable'], 'Windows must be told the engine is gone for good, not restarting');
});

test('an engine that stays up for a minute starts its attempts over, and one that fails to open is tried again', async context => {
	const { service } = await startService(context);
	await crashAndRecover(context, 1000);
	context.mock.timers.tick(60_000);
	await crashAndRecover(context, 1000);
	forks.at(-1).emit('exit', 1);
	await settle();
	context.mock.timers.tick(2000);
	forks.at(-1).emit('message', { ready: false, error: 'StoreLocked' });
	await settle();
	assert.equal(service.isRestarting, true, 'A restart that failed to open was not tried again');
	context.mock.timers.tick(4000);
	assert.equal(forks.length, 5);
	await service.stop();
});

test('stopping the service ends the engine without restarting it', async context => {
	const { service } = await startService(context);
	const stopped = service.stop();
	forks[0].emit('exit', 0);
	await stopped.catch(() => undefined);
	context.mock.timers.tick(60_000);
	await settle();
	assert.equal(forks.length, 1);
	assert.equal(service.isRestarting, false);
});

test('a command that does not answer in time fails alone and says its outcome is unknown', async context => {
	const { service } = await startService(context);
	globalThis.engineCalls = [() => new Promise(() => { })];
	const slow = service.execute({ type: 'startHosting' });
	const failure = assert.rejects(slow, /timed out and may still finish/);
	await settle();
	context.mock.timers.tick(10_000);
	await failure;
	assert.deepEqual(warnings, ['[engine] startHosting did not answer in time; its outcome is unknown.']);
	assert.deepEqual(await service.execute({ type: 'snapshot' }), { type: 'done' }, 'A timed-out command held the next one');
	assert.equal(service.available, true, 'A timeout restarted the engine');
	await service.stop();
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { test } from 'node:test';
import { fakeEngine } from './fixtures.mjs';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

const bundle = await build({
	stdin: { contents: `export { DeviceService } from './src/workbench/services/devices/deviceServiceImpl.js';`, loader: 'ts', resolveDir: root },
	bundle: true, write: false, platform: 'node', format: 'esm', target: 'node22', tsconfig: `${root}tsconfig.json`,
});

const { DeviceService } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

function snapshot(devices) {
	return { type: 'snapshot', snapshot: { devices } };
}

function deferred() {
	let resolve;
	let reject;
	const promise = new Promise((done, fail) => {
		resolve = done;
		reject = fail;
	});
	return { promise, resolve, reject };
}

function device(id, pet_name = null, hostname = null, direct = [], relays = []) {
	return { endpoint_id: id, pet_name, hostname, os: 'linux', presence: 'connected', direct, relays };
}

test('device state maps names, publishes empty results and cannot be mutated by consumers', async () => {
	let reply = snapshot([device('one', 'Personal', 'Host', ['192.0.2.1:7000', '[2001:db8::1]:7000'], ['https://relay.example']), device('two', null, 'Host'), device('three')]);
	const service = new DeviceService(fakeEngine({
		async engineCommand(command) {
			assert.deepEqual(command, { type: 'snapshot' });
			return reply;
		}
	}, true));
	const events = [];
	service.onDidChangeDevices(devices => events.push(devices));
	try {
		await service.refresh(new AbortController().signal);
		assert.deepEqual(service.devices, [
			{ id: 'one', name: 'Personal', os: 'linux', presence: 'connected', hostname: 'Host', direct: 2, relays: 1 },
			{ id: 'two', name: 'Host', os: 'linux', presence: 'connected', hostname: 'Host', direct: 0, relays: 0 },
			{ id: 'three', name: 'three', os: 'linux', presence: 'connected', hostname: null, direct: 0, relays: 0 },
		]);
		assert.throws(() => service.devices.push({ id: 'bad', name: 'bad', os: 'linux', presence: 'connected' }), TypeError);
		assert.throws(() => {
			service.devices[0].name = 'bad';
		}, TypeError);
		reply = snapshot([]);
		await service.refresh(new AbortController().signal);
		assert.deepEqual(service.devices, []);
		assert.equal(events.length, 2);
		assert.equal(events[1], service.devices);
	} finally {
		service.dispose();
	}
});

test('an older snapshot cannot overwrite a newer refresh', async () => {
	const first = deferred();
	const second = deferred();
	const requests = [first, second];
	const service = new DeviceService(fakeEngine({
		engineCommand() {
			return requests.shift().promise;
		}
	}, true));
	let events = 0;
	service.onDidChangeDevices(() => events++);
	try {
		const old = service.refresh(new AbortController().signal);
		const fresh = service.refresh(new AbortController().signal);
		second.resolve(snapshot([device('new')]));
		await fresh;
		first.resolve(snapshot([device('old')]));
		await old;
		assert.deepEqual(service.devices, [{ id: 'new', name: 'new', os: 'linux', presence: 'connected', hostname: null, direct: 0, relays: 0 }]);
		assert.equal(events, 1);
	} finally {
		service.dispose();
	}
});

test('cancellation and disposal fence late replies and errors', async () => {
	for (const stop of ['abort', 'dispose']) {
		for (const outcome of ['resolve', 'reject']) {
			const request = deferred();
			const controller = new AbortController();
			const service = new DeviceService(fakeEngine({
				engineCommand() {
					return request.promise;
				}
			}, true));
			let events = 0;
			service.onDidChangeDevices(() => events++);
			const running = service.refresh(controller.signal);
			if (stop === 'abort') {
				controller.abort();
			} else {
				service.dispose();
			}
			if (outcome === 'resolve') {
				request.resolve(snapshot([device('late')]));
			} else {
				request.reject(new Error('late failure'));
			}
			await running;
			assert.deepEqual(service.devices, []);
			assert.equal(events, 0);
			service.dispose();
		}
	}
});

test('failed refresh preserves the previous snapshot and surfaces current failures', async () => {
	let reply = snapshot([device('saved')]);
	const service = new DeviceService(fakeEngine({
		async engineCommand() {
			if (reply instanceof Error) {
				throw reply;
			}
			return reply;
		}
	}, true));
	try {
		await service.refresh(new AbortController().signal);
		reply = { type: 'ok' };
		await assert.rejects(service.refresh(new AbortController().signal), /Expected engine snapshot/);
		reply = new Error('Transport unavailable');
		await assert.rejects(service.refresh(new AbortController().signal), /Transport unavailable/);
		assert.deepEqual(service.devices, [{ id: 'saved', name: 'saved', os: 'linux', presence: 'connected', hostname: null, direct: 0, relays: 0 }]);
	} finally {
		service.dispose();
	}
});

test('unavailable, cancelled and disposed services never send requests', async () => {
	let calls = 0;
	const service = new DeviceService(fakeEngine({
		async engineCommand() {
			calls++;
			return snapshot([]);
		}
	}, false));
	await assert.rejects(service.refresh(new AbortController().signal), /unavailable/);
	const aborted = new AbortController();
	aborted.abort();
	await service.refresh(aborted.signal);
	service.dispose();
	await service.refresh(new AbortController().signal);
	assert.equal(calls, 0);
});

test('device presence follows engine events, resyncs after gaps and follows engine availability', async () => {
	let state = 'connected';
	let failSnapshot = false;
	const calls = [];
	const engine = fakeEngine({
		async engineCommand(command) {
			calls.push(command.type);
			if (command.type === 'startHosting') {
				return { type: 'done' };
			}
			if (command.type === 'snapshot') {
				if (failSnapshot) {
					throw new Error('Engine busy');
				}
				return snapshot([{ ...device('peer'), presence: state }]);
			}
			throw new Error(command.type);
		}
	}, true);
	const service = new DeviceService(engine);
	let changes = 0;
	service.onDidChangeDevices(() => changes++);
	const settle = async () => {
		for (let step = 0; step < 20; step++) {
			await Promise.resolve();
		}
	};
	try {
		await service.initialize();
		assert.equal(service.devices[0].presence, 'connected');
		engine.fireEvents({ events: [{ type: 'hostLocked', locked: true }], resync: false });
		await settle();
		assert.equal(calls.filter(type => type === 'snapshot').length, 1, 'Unrelated events must not re-read devices');
		engine.fireEvents({ events: [{ type: 'peerRenamed', endpoint_id: 'peer', hostname: null }], resync: false });
		await settle();
		assert.equal(changes, 1, 'Unchanged snapshots must not rebuild the device list');
		state = 'reconnecting';
		engine.fireEvents({ events: [], resync: true });
		await settle();
		assert.equal(service.devices[0].presence, 'reconnecting');
		failSnapshot = true;
		engine.fireEvents({ events: [], resync: true });
		await settle();
		assert.equal(service.devices[0].presence, 'unknown');
		failSnapshot = false;
		engine.setAvailable(false);
		assert.equal(service.state, 'unavailable');
		state = 'disconnected';
		engine.setAvailable(true);
		await settle();
		assert.equal(service.devices[0].presence, 'disconnected');
		assert.equal(service.state, 'ready');
		assert.equal(calls.filter(type => type === 'startHosting').length, 0, 'Hosting is owned by the main process');
	} finally {
		service.dispose();
	}
	assert.equal(engine.eventListeners, 0, 'Disposed service kept its engine event listener');
});

test('device commands use neutral connections and consume a fresh pairing preview on every attempt', async () => {
	const commands = [];
	let attempt = 0;
	const service = new DeviceService(fakeEngine({
		async engineCommand(command) {
			commands.push(command);
			switch (command.type) {
				case 'openPairingFile': return { type: 'pairingPreview', preview: { id: String(++attempt), endpoint_id: 'peer', check: '042', expires_at_ms: '1000' } };
				case 'confirmPairing':
					if (command.digits !== '042') {
						throw new Error('PairingCheckMismatch');
					}
					return { type: 'device', device: device('peer') };
				case 'snapshot': return snapshot([device('peer')]);
				case 'beginConnectServices': return { type: 'session', session: { id: '1' } };
				default: return { type: 'done' };
			}
		}
	}, true));
	try {
		await assert.rejects(service.pair(new Uint8Array([1, 2]), '123'), /PairingCheckMismatch/);
		assert.equal(await service.pair(new Uint8Array([1, 2]), '042'), 'peer');
		assert.deepEqual(commands.filter(command => command.type === 'confirmPairing'), [
			{ type: 'confirmPairing', previewId: '1', digits: '123' },
			{ type: 'confirmPairing', previewId: '2', digits: '042' },
		]);
		assert.equal(commands.some(command => command.type === 'setSharingPolicy'), false);
		await service.connect('peer');
		assert.deepEqual(commands.at(-2), { type: 'beginConnectServices', endpointId: 'peer' });
	} finally {
		service.dispose();
	}
});

test('empty failed reads expose unavailable state and recover without invented devices', async () => {
	let failed = true;
	const service = new DeviceService(fakeEngine({
		async engineCommand() {
			if (failed) {
				throw new Error('Engine unavailable');
			}
			return snapshot([]);
		}
	}, true));
	try {
		assert.equal(service.state, 'loading');
		await assert.rejects(service.refresh(new AbortController().signal));
		assert.equal(service.state, 'unavailable');
		assert.deepEqual(service.devices, []);
		failed = false;
		await service.refresh(new AbortController().signal);
		assert.equal(service.state, 'ready');
	} finally {
		service.dispose();
	}
});

test('reachability changes refresh devices and announce what each computer allows', async () => {
	let presence = 'connected';
	let report = [{ capability: 'camera', status: 'turnedOff' }];
	const engine = fakeEngine({
		async engineCommand(command) {
			if (command.type === 'snapshot') {
				return snapshot([{ ...device('peer'), presence }]);
			}
			if (command.type === 'peerAvailability') {
				assert.equal(command.endpointId, 'peer');
				return { type: 'peerAvailability', report };
			}
			throw new Error(command.type);
		}
	}, true);
	const service = new DeviceService(engine);
	const announced = [];
	service.onDidChangeAvailability(id => announced.push(id));
	const settle = async () => {
		for (let step = 0; step < 20; step++) {
			await Promise.resolve();
		}
	};
	try {
		await service.initialize();
		assert.deepEqual(await service.availability('peer'), [{ capability: 'camera', status: 'turnedOff' }]);
		presence = 'notAccepting';
		report = null;
		engine.fireEvents({ events: [{ type: 'presenceChanged', endpoint_id: 'peer', presence: 'notAccepting' }], resync: false });
		await settle();
		assert.equal(service.devices[0].presence, 'notAccepting');
		assert.equal(await service.availability('peer'), null, 'An unreachable computer reports nothing');
		engine.fireEvents({ events: [{ type: 'peerAvailabilityChanged', endpoint_id: 'peer' }], resync: false });
		await settle();
		assert.deepEqual(announced, ['peer', 'peer']);
	} finally {
		service.dispose();
	}
});

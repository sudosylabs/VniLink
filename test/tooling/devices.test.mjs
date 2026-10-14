import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

const bundle = await build({
	stdin: { contents: `export { DeviceService } from './src/workbench/services/devices/electron-browser/deviceService.js';`, loader: 'ts', resolveDir: root },
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

function device(id, pet_name = null, hostname = null) {
	return { endpoint_id: id, pet_name, hostname };
}

test('device state maps names, publishes empty results and cannot be mutated by consumers', async () => {
	let reply = snapshot([device('one', 'Personal', 'Host'), device('two', null, 'Host'), device('three')]);
	const service = new DeviceService({
		async engineCommand(command) {
			assert.deepEqual(command, { type: 'snapshot' });
			return reply;
		}
	}, true);
	const events = [];
	service.onDidChangeDevices(devices => events.push(devices));
	try {
		await service.refresh(new AbortController().signal);
		assert.deepEqual(service.devices, [{ id: 'one', name: 'Personal' }, { id: 'two', name: 'Host' }, { id: 'three', name: 'three' }]);
		assert.throws(() => service.devices.push({ id: 'bad', name: 'bad' }), TypeError);
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
	const service = new DeviceService({
		engineCommand() {
			return requests.shift().promise;
		}
	}, true);
	let events = 0;
	service.onDidChangeDevices(() => events++);
	try {
		const old = service.refresh(new AbortController().signal);
		const fresh = service.refresh(new AbortController().signal);
		second.resolve(snapshot([device('new')]));
		await fresh;
		first.resolve(snapshot([device('old')]));
		await old;
		assert.deepEqual(service.devices, [{ id: 'new', name: 'new' }]);
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
			const service = new DeviceService({
				engineCommand() {
					return request.promise;
				}
			}, true);
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
	const service = new DeviceService({
		async engineCommand() {
			if (reply instanceof Error) {
				throw reply;
			}
			return reply;
		}
	}, true);
	try {
		await service.refresh(new AbortController().signal);
		reply = { type: 'ok' };
		await assert.rejects(service.refresh(new AbortController().signal), /Expected engine snapshot/);
		reply = new Error('Transport unavailable');
		await assert.rejects(service.refresh(new AbortController().signal), /Transport unavailable/);
		assert.deepEqual(service.devices, [{ id: 'saved', name: 'saved' }]);
	} finally {
		service.dispose();
	}
});

test('unavailable, cancelled and disposed services never send requests', async () => {
	let calls = 0;
	const service = new DeviceService({
		async engineCommand() {
			calls++;
			return snapshot([]);
		}
	}, false);
	await assert.rejects(service.refresh(new AbortController().signal), /unavailable/);
	const aborted = new AbortController();
	aborted.abort();
	await service.refresh(aborted.signal);
	service.dispose();
	await service.refresh(new AbortController().signal);
	assert.equal(calls, 0);
});

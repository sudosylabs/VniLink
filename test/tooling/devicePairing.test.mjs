import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

const bundle = await build({
	stdin: { contents: `export { DevicePairingService } from './src/workbench/services/devices/devicePairingServiceImpl.js'; export { Emitter } from './src/base/common/event.js';`, loader: 'ts', resolveDir: root },
	bundle: true, write: false, platform: 'node', format: 'esm', target: 'node22', tsconfig: `${root}tsconfig.json`,
});

const { DevicePairingService, Emitter } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

function deferred() {
	let resolve;
	const promise = new Promise(done => {
		resolve = done;
	});
	return { promise, resolve };
}

/** Records what pairing asks of the device service; requests and replies are set per test. */
function fakeDevices(overrides = {}) {
	const changed = new Emitter();
	const calls = [];
	const devices = {
		changed,
		calls,
		incoming: null,
		onDidChangeDevices: changed.event,
		exportPairingFile: () => Promise.resolve({ bytes: [1, 2, 3], check: '042', expiresAt: Date.now() + 900_000 }),
		incomingPairing: () => Promise.resolve(devices.incoming),
		approvePairing: digits => {
			calls.push(['approve', digits]);
			devices.incoming = null;
			return Promise.resolve();
		},
		rejectPairing: () => {
			calls.push(['reject']);
			devices.incoming = null;
			return Promise.resolve();
		},
		pair: (bytes, digits) => {
			calls.push(['pair', Array.from(bytes), digits]);
			return Promise.resolve('peer');
		},
		pairingCheck: () => Promise.resolve('731'),
		connect: id => {
			calls.push(['connect', id]);
			return Promise.resolve();
		},
		...overrides,
	};
	return devices;
}

/** Resolves once the service reports a state matching `predicate`. */
function until(service, predicate) {
	if (predicate(service.state)) {
		return Promise.resolve(service.state);
	}
	return new Promise(resolve => {
		const listener = service.onDidChange(state => {
			if (predicate(state)) {
				listener.dispose();
				resolve(state);
			}
		});
	});
}

test('an invitation waits, turns into an approval and pairs on the right digits', async () => {
	const devices = fakeDevices();
	const service = new DevicePairingService(devices);
	try {
		await service.createInvitation();
		assert.equal(service.state.step.kind, 'invitation');
		assert.equal(service.state.step.file.check, '042');
		assert.equal(service.state.busy, false);
		service.reset();
		assert.equal(service.state.step.kind, 'invitation', 'Closing the popover cancelled a live invitation');
		devices.incoming = { endpoint_id: 'peer', check: '731' };
		const approval = await until(service, state => state.step.kind === 'approval');
		assert.equal(approval.step.endpointId, 'peer');
		await service.submitCode('73');
		assert.equal(service.state.message, 'Enter the three digits shown on the other computer.');
		assert.deepEqual(devices.calls, []);
		await service.submitCode('731');
		assert.equal(service.state.step.kind, 'paired');
		assert.deepEqual(devices.calls, [['approve', '731']]);
		service.reset();
		assert.equal(service.state.step.kind, 'idle');
	} finally {
		service.dispose();
	}
});

test('a rejected request ends the invitation', async () => {
	const devices = fakeDevices();
	const service = new DevicePairingService(devices);
	try {
		await service.createInvitation();
		devices.incoming = { endpoint_id: 'peer', check: '731' };
		await until(service, state => state.step.kind === 'approval');
		await service.reject();
		assert.equal(service.state.step.kind, 'ended');
		assert.equal(service.state.message, 'Pairing request rejected.');
		assert.deepEqual(devices.calls, [['reject']]);
	} finally {
		service.dispose();
	}
});

test('a lapsed request returns to the live invitation and a new one asks again', async () => {
	const devices = fakeDevices();
	const service = new DevicePairingService(devices);
	try {
		await service.createInvitation();
		devices.incoming = { endpoint_id: 'peer', check: '731' };
		await until(service, state => state.step.kind === 'approval');
		devices.incoming = null;
		const waiting = await until(service, state => state.step.kind === 'invitation');
		assert.equal(waiting.step.file.check, '042', 'The invitation was replaced instead of kept');
		assert.equal(waiting.message, 'The other computer stopped waiting. It will ask again while this invitation lasts.');
		devices.incoming = { endpoint_id: 'peer', check: '731' };
		await until(service, state => state.step.kind === 'approval');
		await service.submitCode('731');
		assert.equal(service.state.step.kind, 'paired');
		assert.deepEqual(devices.calls, [['approve', '731']]);
	} finally {
		service.dispose();
	}
});

test('an invitation expires on its own', async () => {
	const service = new DevicePairingService(fakeDevices({
		exportPairingFile: () => Promise.resolve({ bytes: [1], check: '042', expiresAt: Date.now() + 20 }),
	}));
	try {
		await service.createInvitation();
		const ended = await until(service, state => state.step.kind === 'ended');
		assert.equal(ended.message, 'The invitation expired. Create a new invitation to pair.');
	} finally {
		service.dispose();
	}
});

test('opening an invitation checks the file, pairs and shows the reverse code until the link connects', async () => {
	const devices = fakeDevices();
	const service = new DevicePairingService(devices);
	try {
		await service.openInvitation(new File([], 'empty.vnl'));
		assert.equal(service.state.step.kind, 'idle');
		assert.equal(service.state.message, 'Choose a valid .vnl invitation smaller than 64 KB.');
		await service.openInvitation(new File([new Uint8Array(65_537)], 'large.vnl'));
		assert.equal(service.state.message, 'Choose a valid .vnl invitation smaller than 64 KB.');
		await service.openInvitation(new File([new Uint8Array([86, 78])], 'office.vnl'));
		assert.deepEqual(service.state.step, { kind: 'opened', fileName: 'office.vnl' });
		await service.submitCode('042');
		assert.deepEqual(service.state.step, { kind: 'reverseCode', code: '731', endpointId: 'peer' });
		assert.deepEqual(devices.calls, [['pair', [86, 78], '042'], ['connect', 'peer']]);
		devices.changed.fire([{ id: 'peer', presence: 'connecting' }]);
		assert.equal(service.state.step.kind, 'reverseCode');
		devices.changed.fire([{ id: 'peer', presence: 'connected' }]);
		assert.equal(service.state.step.kind, 'paired');
	} finally {
		service.dispose();
	}
});

test('a failed pairing keeps the opened invitation for another try', async () => {
	const service = new DevicePairingService(fakeDevices({ pair: () => Promise.reject(new Error('The code does not match.')) }));
	try {
		await service.openInvitation(new File([new Uint8Array([1])], 'office.vnl'));
		await service.submitCode('999');
		assert.equal(service.state.step.kind, 'opened');
		assert.equal(service.state.message, 'The code does not match.');
		assert.equal(service.state.busy, false);
	} finally {
		service.dispose();
	}
});

test('commands are not started twice and late replies after disposal are dropped', async () => {
	const pending = deferred();
	let exports = 0;
	const service = new DevicePairingService(fakeDevices({
		exportPairingFile: () => {
			exports++;
			return pending.promise;
		},
	}));
	const events = [];
	service.onDidChange(state => events.push(state));
	const first = service.createInvitation();
	assert.equal(service.state.busy, true);
	await service.createInvitation();
	assert.equal(exports, 1, 'A busy flow started a second invitation');
	service.dispose();
	const seen = events.length;
	pending.resolve({ bytes: [1], check: '042', expiresAt: Date.now() + 900_000 });
	await first;
	assert.equal(events.length, seen, 'A disposed service announced a late invitation');
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { DeferredPromise, timeout } from '../../../../../base/common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import type { EngineCommand, EngineReply, RemoteCapabilityStatus } from '../../../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../../../platform/engine/common/engineService.js';
import { engineDevice, snapshotReply, TestEngineService, type EngineDevice } from '../../../../../platform/engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import type { DeviceSummary } from '../../deviceService.js';
import { DeviceService } from '../../deviceServiceImpl.js';

/** The summary a device with nothing but an id shows. */
function summary(id: string, overrides: Partial<DeviceSummary> = {}): DeviceSummary {
	return { id, name: id, os: 'linux', presence: 'connected', hostname: null, direct: 0, relays: 0, ...overrides };
}

suite('DeviceService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let engine: TestEngineService;

	/** The service under test, built as the workbench builds it, on `engine`. */
	function createService(): DeviceService {
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IEngineService, engine);
		return disposables.add(instantiationService.createInstance(DeviceService));
	}

	/** An engine that answers every snapshot with `devices()`, read when asked. */
	function snapshots(devices: () => readonly EngineDevice[]): (command: EngineCommand) => EngineReply {
		return command => {
			if (command.type !== 'snapshot') {
				throw new Error(`Unexpected command ${command.type}`);
			}
			return snapshotReply(devices());
		};
	}
	setup(() => {
		engine = disposables.add(new TestEngineService(snapshots(() => [])));
	});
	test('names devices by pet name, then host name, then id, and publishes an empty list', async () => {
		let devices = [
			engineDevice('one', { pet_name: 'Personal', hostname: 'Host', direct: ['192.0.2.1:7000', '[2001:db8::1]:7000'], relays: ['https://relay.example'] }),
			engineDevice('two', { hostname: 'Host' }),
			engineDevice('three'),
		];
		engine.handler = snapshots(() => devices);
		const service = createService();
		const published: (readonly DeviceSummary[])[] = [];
		disposables.add(service.onDidChangeDevices(list => published.push(list)));
		await service.refresh(new AbortController().signal);
		assert.deepStrictEqual(service.devices, [
			summary('one', { name: 'Personal', hostname: 'Host', direct: 2, relays: 1 }),
			summary('two', { name: 'Host', hostname: 'Host' }),
			summary('three'),
		]);
		devices = [];
		await service.refresh(new AbortController().signal);
		assert.deepStrictEqual(service.devices, []);
		assert.strictEqual(published.length, 2);
		assert.strictEqual(published[1], service.devices, 'the event carries the list the service now holds');
	});
	test('consumers cannot change the device list or its entries', async () => {
		engine.handler = snapshots(() => [engineDevice('one')]);
		const service = createService();
		await service.refresh(new AbortController().signal);
		assert.throws(() => (service.devices as DeviceSummary[]).push(summary('bad')), TypeError);
		assert.throws(() => {
			(service.devices[0] as { name: string }).name = 'bad';
		}, TypeError);
	});
	test('an older snapshot cannot overwrite a newer refresh', async () => {
		const first = new DeferredPromise<EngineReply>();
		const second = new DeferredPromise<EngineReply>();
		const requests = [first, second];
		engine.handler = () => requests.shift()?.p ?? Promise.reject(new Error('No request expected'));
		const service = createService();
		let changes = 0;
		disposables.add(service.onDidChangeDevices(() => changes++));
		const old = service.refresh(new AbortController().signal);
		const fresh = service.refresh(new AbortController().signal);
		await second.complete(snapshotReply([engineDevice('new')]));
		await fresh;
		await first.complete(snapshotReply([engineDevice('old')]));
		await old;
		assert.deepStrictEqual(service.devices, [summary('new')]);
		assert.strictEqual(changes, 1);
	});
	for (const stop of ['abort', 'dispose'] as const) {
		for (const outcome of ['reply', 'failure'] as const) {
			test(`a late ${outcome} after ${stop === 'abort' ? 'cancellation' : 'disposal'} changes nothing`, async () => {
				const request = new DeferredPromise<EngineReply>();
				engine.handler = () => request.p;
				const controller = new AbortController();
				const service = createService();
				let changes = 0;
				disposables.add(service.onDidChangeDevices(() => changes++));
				const running = service.refresh(controller.signal);
				if (stop === 'abort') {
					controller.abort();
				} else {
					service.dispose();
				}
				if (outcome === 'reply') {
					await request.complete(snapshotReply([engineDevice('late')]));
				} else {
					await request.error(new Error('late failure'));
				}
				await running;
				assert.deepStrictEqual(service.devices, []);
				assert.strictEqual(changes, 0);
			});
		}
	}
	test('a failed refresh keeps the previous devices and reports the failure', async () => {
		let reply: EngineReply | Error = snapshotReply([engineDevice('saved')]);
		engine.handler = () => {
			if (reply instanceof Error) {
				throw reply;
			}
			return reply;
		};
		const service = createService();
		await service.refresh(new AbortController().signal);
		reply = { type: 'swap', enabled: false };
		await assert.rejects(service.refresh(new AbortController().signal), /Expected engine snapshot/);
		reply = new Error('Transport unavailable');
		await assert.rejects(service.refresh(new AbortController().signal), /Transport unavailable/);
		assert.deepStrictEqual(service.devices, [summary('saved')]);
	});
	test('an unavailable, cancelled or disposed service sends no request', async () => {
		engine.setAvailable(false);
		const service = createService();
		await assert.rejects(service.refresh(new AbortController().signal), /unavailable/);
		await service.refresh(AbortSignal.abort());
		service.dispose();
		await service.refresh(new AbortController().signal);
		assert.deepStrictEqual(engine.commands, []);
	});
	test('a failed first read shows the service unavailable, without invented devices, until a read succeeds', async () => {
		let failed = true;
		engine.handler = () => {
			if (failed) {
				throw new Error('Engine unavailable');
			}
			return snapshotReply([]);
		};
		const service = createService();
		assert.strictEqual(service.state, 'loading');
		await assert.rejects(service.refresh(new AbortController().signal));
		assert.strictEqual(service.state, 'unavailable');
		assert.deepStrictEqual(service.devices, []);
		failed = false;
		await service.refresh(new AbortController().signal);
		assert.strictEqual(service.state, 'ready');
	});
	suite('engine events', () => {
		let presence: DeviceSummary['presence'] & EngineDevice['presence'];
		let snapshotFails: boolean;
		setup(() => {
			presence = 'connected';
			snapshotFails = false;
			engine.handler = command => {
				if (command.type !== 'snapshot') {
					throw new Error(`Unexpected command ${command.type}`);
				}
				if (snapshotFails) {
					throw new Error('Engine busy');
				}
				return snapshotReply([engineDevice('peer', { presence })]);
			};
		});

		/** Lets the refresh an event started finish: every promise callback runs before the next task. */
		function settled(): Promise<void> {
			return timeout(0);
		}
		const snapshotsRead = (): number => engine.commands.filter(command => command.type === 'snapshot').length;
		test('an unrelated event does not re-read devices', async () => {
			const service = createService();
			await service.initialize();
			engine.fireEvents({ events: [{ type: 'hostLocked', locked: true }], resync: false });
			await settled();
			assert.strictEqual(snapshotsRead(), 1);
		});
		test('a device event re-reads devices, and an unchanged list is not published again', async () => {
			const service = createService();
			let changes = 0;
			disposables.add(service.onDidChangeDevices(() => changes++));
			await service.initialize();
			engine.fireEvents({ events: [{ type: 'peerRenamed', endpoint_id: 'peer', hostname: null }], resync: false });
			await settled();
			assert.strictEqual(snapshotsRead(), 2);
			assert.strictEqual(changes, 1);
		});
		test('a resync after missed events reads the current presence', async () => {
			const service = createService();
			await service.initialize();
			presence = 'reconnecting';
			engine.fireEvents({ events: [], resync: true });
			await settled();
			assert.strictEqual(service.devices[0].presence, 'reconnecting');
		});
		test('a resync that cannot read devices marks their presence unknown', async () => {
			const service = createService();
			await service.initialize();
			snapshotFails = true;
			engine.fireEvents({ events: [], resync: true });
			await settled();
			assert.strictEqual(service.devices[0].presence, 'unknown');
		});
		test('the service follows engine availability, reading devices again when the engine returns', async () => {
			const service = createService();
			await service.initialize();
			engine.setAvailable(false);
			assert.strictEqual(service.state, 'unavailable');
			presence = 'disconnected';
			engine.setAvailable(true);
			await settled();
			assert.strictEqual(service.devices[0].presence, 'disconnected');
			assert.strictEqual(service.state, 'ready');
		});
		test('the window never starts hosting: main owns it', async () => {
			const service = createService();
			await service.initialize();
			engine.fireEvents({ events: [], resync: true });
			await settled();
			assert.deepStrictEqual(engine.commands.filter(command => command.type === 'startHosting'), []);
		});
		test('disposing the service stops its engine event listener', async () => {
			const service = createService();
			await service.initialize();
			assert.strictEqual(engine.hasEventListeners, true);
			service.dispose();
			assert.strictEqual(engine.hasEventListeners, false);
		});
		test('a reachability change re-reads devices and announces what the computer allows', async () => {
			let report: RemoteCapabilityStatus[] | null = [{ capability: 'camera', status: 'turnedOff' }];
			const snapshot = engine.handler;
			engine.handler = command => {
				if (command.type === 'peerAvailability') {
					assert.strictEqual(command.endpointId, 'peer');
					return { type: 'peerAvailability', report };
				}
				return snapshot(command);
			};
			const service = createService();
			const announced: string[] = [];
			disposables.add(service.onDidChangeAvailability(id => announced.push(id)));
			await service.initialize();
			assert.deepStrictEqual(await service.availability('peer'), [{ capability: 'camera', status: 'turnedOff' }]);
			presence = 'notAccepting';
			report = null;
			engine.fireEvents({ events: [{ type: 'presenceChanged', endpoint_id: 'peer', presence: 'notAccepting' }], resync: false });
			await settled();
			assert.strictEqual(service.devices[0].presence, 'notAccepting');
			assert.strictEqual(await service.availability('peer'), null, 'an unreachable computer reports nothing');
			engine.fireEvents({ events: [{ type: 'peerAvailabilityChanged', endpoint_id: 'peer' }], resync: false });
			await settled();
			assert.deepStrictEqual(announced, ['peer', 'peer']);
		});
	});
	suite('commands', () => {
		test('pairing opens a fresh preview for every attempt and confirms with the typed digits', async () => {
			let attempt = 0;
			engine.handler = command => {
				if (command.type === 'openPairingFile') {
					return { type: 'pairingPreview', preview: { id: String(++attempt), endpoint_id: 'peer', check: '042517', expires_at_ms: '1000' } };
				}
				if (command.type === 'confirmPairing') {
					if (command.digits !== '042517') {
						throw new Error('PairingCheckMismatch');
					}
					return { type: 'device', device: engineDevice('peer') };
				}
				if (command.type === 'snapshot') {
					return snapshotReply([engineDevice('peer')]);
				}
				throw new Error(`Unexpected command ${command.type}`);
			};
			const service = createService();
			await assert.rejects(service.pair(new Uint8Array([1, 2]), '123'), /PairingCheckMismatch/);
			assert.strictEqual(await service.pair(new Uint8Array([1, 2]), '042517'), 'peer');
			assert.deepStrictEqual(engine.commands.filter(command => command.type === 'confirmPairing'), [
				{ type: 'confirmPairing', previewId: '1', digits: '123' },
				{ type: 'confirmPairing', previewId: '2', digits: '042517' },
			]);
			assert.strictEqual(engine.commands.some(command => command.type === 'setSharingPolicy'), false, 'pairing grants nothing by itself');
		});
		test('connecting opens a neutral connection, with no service', async () => {
			engine.handler = command => {
				if (command.type === 'beginConnectServices') {
					return { type: 'session', session: { id: '1', generation: '1', role: 'Controller', phase: 'Active', peer_id: 'peer' } };
				}
				if (command.type === 'snapshot') {
					return snapshotReply([engineDevice('peer')]);
				}
				throw new Error(`Unexpected command ${command.type}`);
			};
			const service = createService();
			await service.connect('peer');
			assert.deepStrictEqual(engine.commands.map(command => command.type), ['beginConnectServices', 'snapshot']);
			assert.deepStrictEqual(engine.commands[0], { type: 'beginConnectServices', endpointId: 'peer' });
		});
	});
});

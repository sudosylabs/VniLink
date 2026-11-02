// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { DeferredPromise } from '../../../../../base/common/async.js';
import { Event } from '../../../../../base/common/event.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../../base/test/common/virtualScheduling/index.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import type { PairingState, PairingStep } from '../../devicePairingService.js';
import { DevicePairingService } from '../../devicePairingServiceImpl.js';
import { IDeviceService, type PairingFile } from '../../deviceService.js';
import { TestDeviceService } from '../common/testDeviceService.js';

type Step<K extends PairingStep['kind']> = Extract<PairingStep, { kind: K }>;

/** The step the service shows, which must be of `kind`. */
function step<K extends PairingStep['kind']>(service: DevicePairingService, kind: K): Step<K> {
	const current = service.state.step;
	assert.strictEqual(current.kind, kind);
	return current as Step<K>;
}

/** Resolves with the service's state once it shows a step of `kind`. */
function until(service: DevicePairingService, kind: PairingStep['kind']): Promise<PairingState> {
	if (service.state.step.kind === kind) {
		return Promise.resolve(service.state);
	}
	return Event.toPromise(Event.filter(service.onDidChange, state => state.step.kind === kind));
}

suite('DevicePairingService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let devices: TestDeviceService;
	function createService(): DevicePairingService {
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IDeviceService, devices);
		return disposables.add(instantiationService.createInstance(DevicePairingService));
	}

	/**
	 * Runs `fn` on virtual time against a service, then disposes the service so
	 * a live invitation stops polling before virtual time drains.
	 */
	function withService(fn: (service: DevicePairingService) => Promise<void>): Promise<void> {
		return runWithFakedTimers({}, async () => {
			const service = createService();
			try {
				await fn(service);
			} finally {
				service.dispose();
			}
		});
	}

	/** Creates an invitation and waits for the other computer's request to be shown. */
	async function requested(service: DevicePairingService): Promise<void> {
		await service.createInvitation();
		devices.incoming = { endpoint_id: 'peer' };
		await until(service, 'approval');
	}
	setup(() => {
		devices = disposables.add(new TestDeviceService());
	});
	suite('invitation', () => {
		test('a new invitation shows its check digits once it is ready', () => withService(async service => {
			await service.createInvitation();
			assert.strictEqual(step(service, 'invitation').file.check, '042517');
			assert.strictEqual(service.state.busy, false);
		}));
		test('closing the popover keeps a live invitation', () => withService(async service => {
			await service.createInvitation();
			service.reset();
			assert.strictEqual(service.state.step.kind, 'invitation');
		}));
		test('a request from the other computer turns the invitation into an approval for it', () => withService(async service => {
			await requested(service);
			assert.strictEqual(step(service, 'approval').endpointId, 'peer');
		}));
		test('a code that is not six digits asks for six digits and approves nothing', () => withService(async service => {
			await requested(service);
			await service.submitCode('73');
			assert.strictEqual(service.state.message, 'Enter the six digits shown on the other computer.');
			assert.deepStrictEqual(devices.calls, []);
		}));
		test('six digits approve the request and pair', () => withService(async service => {
			await requested(service);
			await service.submitCode('731904');
			assert.strictEqual(service.state.step.kind, 'paired');
			assert.deepStrictEqual(devices.calls, [{ type: 'approve', digits: '731904' }]);
		}));
		test('resetting after pairing returns to idle', () => withService(async service => {
			await requested(service);
			await service.submitCode('731904');
			service.reset();
			assert.strictEqual(service.state.step.kind, 'idle');
		}));
		test('a rejected request ends the invitation', () => withService(async service => {
			await requested(service);
			await service.reject();
			assert.strictEqual(service.state.step.kind, 'ended');
			assert.strictEqual(service.state.message, 'Pairing request rejected.');
			assert.deepStrictEqual(devices.calls, [{ type: 'reject' }]);
		}));
		test('a lapsed request returns to the same live invitation and says the other computer will ask again', () => withService(async service => {
			await requested(service);
			devices.incoming = null;
			const waiting = await until(service, 'invitation');
			assert.strictEqual((waiting.step as Step<'invitation'>).file.check, '042517', 'the invitation was kept, not replaced');
			assert.strictEqual(waiting.message, 'The other computer stopped waiting. It will ask again while this invitation lasts.');
		}));
		test('a request after a lapsed one asks again and can be approved', () => withService(async service => {
			await requested(service);
			devices.incoming = null;
			await until(service, 'invitation');
			devices.incoming = { endpoint_id: 'peer' };
			await until(service, 'approval');
			await service.submitCode('731904');
			assert.strictEqual(service.state.step.kind, 'paired');
			assert.deepStrictEqual(devices.calls, [{ type: 'approve', digits: '731904' }]);
		}));
		test('an invitation expires on its own', () => withService(async service => {
			devices.exportPairingFile = () => Promise.resolve({ bytes: [1], check: '042517', expiresAt: Date.now() + 20 });
			await service.createInvitation();
			const ended = await until(service, 'ended');
			assert.strictEqual(ended.message, 'The invitation expired. Create a new invitation to pair.');
		}));
	});
	suite('opened invitation', () => {
		for (const [name, file] of [['an empty', new File([], 'empty.vnl')], ['an oversized', new File([new Uint8Array(65_537)], 'large.vnl')]] as const) {
			test(`${name} invitation file is refused`, async () => {
				const service = createService();
				await service.openInvitation(file);
				assert.strictEqual(service.state.step.kind, 'idle');
				assert.strictEqual(service.state.message, 'Choose a valid .vnl invitation smaller than 64 KB.');
			});
		}
		/** A service that has opened a two-byte invitation `office.vnl`. */
		async function opened(): Promise<DevicePairingService> {
			const service = createService();
			await service.openInvitation(new File([new Uint8Array([86, 78])], 'office.vnl'));
			return service;
		}
		test('an opened invitation shows its file name', async () => {
			const service = await opened();
			assert.deepStrictEqual(service.state.step, { kind: 'opened', fileName: 'office.vnl' });
		});
		test('the right digits pair with the file, connect and wait for the reverse code', async () => {
			const service = await opened();
			await service.submitCode('042517');
			assert.deepStrictEqual(service.state.step, { kind: 'reverseCode', code: null, endpointId: 'peer' });
			assert.deepStrictEqual(devices.calls, [{ type: 'pair', bytes: [86, 78], digits: '042517' }, { type: 'connect', id: 'peer' }]);
		});
		test('digits from another computer are not shown', async () => {
			const service = await opened();
			await service.submitCode('042517');
			devices.fireControlCheck({ endpointId: 'other', check: '000000' });
			assert.strictEqual(step(service, 'reverseCode').code, null);
		});
		test('the reverse code shows the digits the paired computer sends', async () => {
			const service = await opened();
			await service.submitCode('042517');
			devices.fireControlCheck({ endpointId: 'peer', check: '918273' });
			assert.deepStrictEqual(service.state.step, { kind: 'reverseCode', code: '918273', endpointId: 'peer' });
		});
		test('pairing completes only once the paired computer is connected', async () => {
			const service = await opened();
			await service.submitCode('042517');
			const peer = { id: 'peer', name: 'peer', os: null, hostname: null, direct: 0, relays: 0 };
			devices.fireDevices([{ ...peer, presence: 'connecting' }]);
			assert.strictEqual(service.state.step.kind, 'reverseCode');
			devices.fireDevices([{ ...peer, presence: 'connected' }]);
			assert.strictEqual(service.state.step.kind, 'paired');
		});
		test('a failed pairing keeps the opened invitation for another try', async () => {
			devices.pair = () => Promise.reject(new Error('The code does not match.'));
			const service = await opened();
			await service.submitCode('999999');
			assert.strictEqual(service.state.step.kind, 'opened');
			assert.strictEqual(service.state.message, 'The code does not match.');
			assert.strictEqual(service.state.busy, false);
		});
	});
	suite('running commands', () => {
		let pending: DeferredPromise<PairingFile>;
		let exports: number;
		setup(() => {
			pending = new DeferredPromise<PairingFile>();
			exports = 0;
			devices.exportPairingFile = () => {
				exports++;
				return pending.p;
			};
		});
		test('a command is not started again while it runs', async () => {
			const service = createService();
			const first = service.createInvitation();
			assert.strictEqual(service.state.busy, true);
			await service.createInvitation();
			assert.strictEqual(exports, 1);
			service.dispose();
			await pending.complete({ bytes: [1], check: '042517', expiresAt: Date.now() + 900_000 });
			await first;
		});
		test('a reply that arrives after disposal is not announced', async () => {
			const service = createService();
			const states: PairingState[] = [];
			disposables.add(service.onDidChange(state => states.push(state)));
			const first = service.createInvitation();
			service.dispose();
			const seen = states.length;
			await pending.complete({ bytes: [1], check: '042517', expiresAt: Date.now() + 900_000 });
			await first;
			assert.strictEqual(states.length, seen);
			assert.strictEqual(service.state.step.kind, 'idle', 'the late invitation was not taken up');
		});
	});
});

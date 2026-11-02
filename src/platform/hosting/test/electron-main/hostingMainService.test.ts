// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../base/common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { IConfigurationMainService } from '../../../configuration/electron-main/configurationMainService.js';
import { TestConfigurationMainService } from '../../../configuration/test/common/testConfigurationMainService.js';
import type { EngineCommand, EngineReply, SessionInfo } from '../../../engine/common/engine.js';
import { IEngineMainService } from '../../../engine/electron-main/engineMainService.js';
import { engineDevice, snapshotReply, TestEngineService, type EngineDevice } from '../../../engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../log/common/log.js';
import { HostingMainService, type HostingStatus } from '../../electron-main/hostingMainService.js';

type ServiceSummary = Extract<EngineReply, { type: 'services' }>['services'][number];

/** What the engine hosts; the test changes it and the engine's replies follow. */
interface HostedState {
	hosting: boolean;
	sessions: SessionInfo[];
	services: ServiceSummary[];
	devices: EngineDevice[];
}

const ACCEPT = 'application.acceptConnections';

function session(id: string, peer: string, phase = 'Active'): SessionInfo {
	return { id, generation: '1', role: 'Host', phase, peer_id: peer };
}

function service(connectionId: string, phase: ServiceSummary['phase'] = 'Active'): ServiceSummary {
	return { id: `s${connectionId}`, connectionId, generation: '1', peer_id: 'x', kind: 'Desktop', phase };
}

/** Lets the service finish what the last change started; the engine answers at once. */
function settle(): Promise<void> {
	return timeout(0);
}

suite('HostingMainService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let state: HostedState;
	/** Whether the engine refuses `startHosting`. */
	let refuseStart: boolean;
	let engine: TestEngineService;
	let configuration: TestConfigurationMainService;
	setup(() => {
		state = { hosting: false, sessions: [], services: [], devices: [engineDevice('a', { pet_name: 'Work laptop', hostname: 'host-a' }), engineDevice('b', { hostname: 'host-b' })] };
		refuseStart = false;
		engine = disposables.add(new TestEngineService(command => reply(command)));
		configuration = disposables.add(new TestConfigurationMainService({ [ACCEPT]: true }));
	});
	function reply(command: EngineCommand): EngineReply {
		if (command.type === 'startHosting') {
			if (refuseStart) {
				throw new Error('Engine busy');
			}
			state.hosting = true;
			return { type: 'done' };
		}
		if (command.type === 'stopHosting') {
			state.hosting = false;
			state.sessions = [];
			return { type: 'done' };
		}
		if (command.type === 'stopHostedPeer') {
			state.sessions = state.sessions.filter(candidate => candidate.id !== command.sessionId);
			state.services = state.services.filter(candidate => candidate.connectionId !== command.sessionId);
			return { type: 'done' };
		}
		if (command.type === 'hostedSessions') {
			return { type: 'sessions', sessions: state.sessions };
		}
		if (command.type === 'hostedServices') {
			return { type: 'services', services: state.services };
		}
		if (command.type === 'snapshot') {
			const snapshot = snapshotReply(state.devices);
			return { ...snapshot, snapshot: { ...snapshot.snapshot, hosting: state.hosting } };
		}
		throw new Error(`Unexpected command ${command.type}`);
	}
	function createService(): HostingMainService {
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IEngineMainService, engine);
		instantiationService.stub(IConfigurationMainService, configuration);
		instantiationService.stub(ILogService, disposables.add(new NullLogService()));
		return disposables.add(instantiationService.createInstance(HostingMainService));
	}
	async function startService(): Promise<HostingMainService> {
		const hosting = createService();
		hosting.start();
		await settle();
		return hosting;
	}

	/** Changes what the engine hosts and announces it, as a hosted service changing would. */
	async function connect(sessions: SessionInfo[], services: ServiceSummary[]): Promise<void> {
		state.sessions = sessions;
		state.services = services;
		engine.fireEvents({ events: [{ type: 'hostedServiceChanged', service: services[0] ?? service('0') }], resync: false });
		await settle();
	}
	function count(type: EngineCommand['type']): number {
		return engine.commands.filter(command => command.type === type).length;
	}
	test('starting twice starts hosting once', async () => {
		const hosting = createService();
		hosting.start();
		hosting.start();
		await settle();
		assert.deepStrictEqual(engine.commands.map(command => command.type).filter(type => type.endsWith('Hosting')), ['startHosting']);
	});
	test('a started service hosts with no one connected', async () => {
		const hosting = await startService();
		assert.deepStrictEqual(hosting.status, { available: true, accepting: true, hosting: true, sessions: [] } satisfies HostingStatus);
	});
	test('idle links from paired computers are not someone connected', async () => {
		const hosting = await startService();
		await connect([session('1', 'a'), session('4', 'd')], []);
		assert.deepStrictEqual(hosting.status.sessions, []);
	});
	test('a connected device is named by its pet name, then its host name', async () => {
		const hosting = await startService();
		await connect([session('1', 'a'), session('2', 'b', 'Negotiating')], [service('1'), service('2', 'Waiting')]);
		assert.deepStrictEqual(hosting.status.sessions, [{ id: '1', name: 'Work laptop', phase: 'Active' }, { id: '2', name: 'host-b', phase: 'Negotiating' }]);
	});
	test('a stopping session or a closed service is not someone connected', async () => {
		const hosting = await startService();
		await connect([session('3', 'c', 'Stopping'), session('4', 'd')], [service('3'), service('4', { Closed: { cause: 'Local' } })]);
		assert.deepStrictEqual(hosting.status.sessions, []);
	});
	test('events unrelated to hosting do not read the engine again', async () => {
		await startService();
		const reads = count('snapshot');
		engine.fireEvents({ events: [{ type: 'inputFailed', id: '1', message: 'denied' }], resync: false });
		await settle();
		assert.strictEqual(count('snapshot'), reads);
	});
	test('disconnecting a device ends only its session', async () => {
		const hosting = await startService();
		await connect([session('1', 'a'), session('2', 'b')], [service('1'), service('2')]);
		await hosting.disconnect('1');
		assert.deepStrictEqual(hosting.status.sessions.map(peer => peer.id), ['2']);
	});
	test('disconnecting a device leaves hosting on', async () => {
		const hosting = await startService();
		await connect([session('1', 'a')], [service('1')]);
		await hosting.disconnect('1');
		assert.strictEqual(state.hosting, true);
	});
	test('pausing saves the choice', async () => {
		const hosting = await startService();
		await hosting.setAccepting(false);
		assert.deepStrictEqual(configuration.updates, [[ACCEPT, false]]);
	});
	test('pausing stops hosting and disconnects everyone', async () => {
		const hosting = await startService();
		await connect([session('1', 'a')], [service('1')]);
		await hosting.setAccepting(false);
		await settle();
		assert.strictEqual(state.hosting, false);
		assert.deepStrictEqual(hosting.status, { available: true, accepting: false, hosting: false, sessions: [] } satisfies HostingStatus);
	});
	test('a resync does not undo a pause', async () => {
		const hosting = await startService();
		await hosting.setAccepting(false);
		await settle();
		engine.fireEvents({ events: [], resync: true });
		await settle();
		assert.strictEqual(state.hosting, false);
	});
	test('an engine that stops answering is reported unavailable', async () => {
		const hosting = await startService();
		engine.setAvailable(false);
		assert.strictEqual(hosting.status.available, false);
	});
	test('an engine that comes back while paused stays paused', async () => {
		const hosting = await startService();
		await hosting.setAccepting(false);
		await settle();
		engine.setAvailable(false);
		engine.setAvailable(true);
		await settle();
		assert.strictEqual(state.hosting, false);
	});
	test('resuming starts hosting again', async () => {
		const hosting = await startService();
		await hosting.setAccepting(false);
		await settle();
		await hosting.setAccepting(true);
		await settle();
		assert.strictEqual(state.hosting, true);
	});
	test('the status is announced only when it changes', async () => {
		const hosting = createService();
		const statuses: HostingStatus[] = [];
		disposables.add(hosting.onDidChangeStatus(status => statuses.push(status)));
		hosting.start();
		await settle();
		engine.fireEvents({ events: [], resync: true });
		await settle();
		await hosting.setAccepting(false);
		await settle();
		engine.fireEvents({ events: [], resync: true });
		await settle();
		assert.deepStrictEqual(statuses.map(status => [status.accepting, status.hosting]), [[true, true], [false, false]]);
	});
	test('a disposed service stops listening to the engine', async () => {
		const hosting = await startService();
		hosting.dispose();
		assert.strictEqual(engine.hasEventListeners, false);
	});
	test('a disposed service stops listening to settings', async () => {
		const hosting = await startService();
		hosting.dispose();
		await hosting.setAccepting(false);
		await settle();
		assert.strictEqual(count('stopHosting'), 0);
	});
	test('a saved pause holds from the first start', async () => {
		state.hosting = true;
		configuration.setValue(ACCEPT, false);
		await startService();
		assert.deepStrictEqual(engine.commands.map(command => command.type).filter(type => type.endsWith('Hosting')), ['stopHosting']);
	});
	test('a refused start leaves hosting off', async () => {
		refuseStart = true;
		const hosting = await startService();
		assert.strictEqual(hosting.status.hosting, false);
	});
	test('a refused start is not retried on ordinary events', async () => {
		refuseStart = true;
		await startService();
		engine.fireEvents({ events: [{ type: 'sessionChanged', ...session('1', 'a') }], resync: false });
		await settle();
		assert.strictEqual(count('startHosting'), 1);
	});
	test('a refused start is retried on the next resync', async () => {
		refuseStart = true;
		const hosting = await startService();
		refuseStart = false;
		engine.fireEvents({ events: [], resync: true });
		await settle();
		assert.strictEqual(count('startHosting'), 2);
		assert.strictEqual(hosting.status.hosting, true);
	});
});

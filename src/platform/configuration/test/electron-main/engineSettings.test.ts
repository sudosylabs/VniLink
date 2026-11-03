// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../base/common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import type { EngineCommand, EngineReply, PowerPolicy, RelayInfo, RelayInput, SharingPolicy, SystemPolicy, SystemSupport, TerminalPolicy } from '../../../engine/common/engine.js';
import { TestEngineMainService } from '../../../engine/test/electron-main/testEngineMainService.js';
import { EngineSettings } from '../../electron-main/engineSettings.js';

/** A relay as the settings editor submits it. */
function relay(url: string, authenticated = false, replacementToken = ''): RelayInput {
	return { url, authenticated, replacementToken };
}

/**
 * The policies an engine holds, answering their reads and writes. `failWrite`
 * refuses every change and `failRead` every read; `loseReadAfterWrite` makes
 * the sharing policy unreadable once a sharing change was written.
 */
class EnginePolicies {
	public power: PowerPolicy = {
		incoming_display: false, incoming_system: true, outgoing_display: false, outgoing_system: false,
		incoming_idle_secs: 90, outgoing_idle_secs: 0, lock_after_control: false,
	};
	public terminal: TerminalPolicy = {
		allow_persistence: true, max_sessions: 16, max_sessions_per_peer: 16, history_bytes: 1048576, replay_bytes: 8192, detached_timeout_secs: 300,
	};
	public sharing: SharingPolicy = {
		view: false, control: false, clipboard: true, files: false, terminal: false,
		camera: false, system_audio: false, microphone: false, recording: false, system_actions: false, unattended: false,
	};
	public system: SystemPolicy = { allow_lock: true, allow_restart: false, allow_shutdown: false, allow_privacy: false };
	public readonly support: SystemSupport = { lock: true, restart: false, shutdown: true, privacy: true };
	public relays: RelayInfo[] = [];
	public computerName: string | null = null;
	public failWrite = false;
	public failRead = false;
	public loseReadAfterWrite = false;
	private sharingWritten = false;
	public readonly handler = (command: EngineCommand): EngineReply => {
		if (command.type.startsWith('set') || command.type === 'saveCustomRelays') {
			if (this.failWrite) {
				throw new Error('Engine refused');
			}
		} else if (this.failRead) {
			throw new Error('Engine stopped');
		}
		if (command.type === 'powerPolicy') {
			return { type: 'powerPolicy', policy: this.power };
		} else if (command.type === 'terminalPolicy') {
			return { type: 'terminalPolicy', policy: this.terminal };
		} else if (command.type === 'sharingPolicy') {
			if (this.loseReadAfterWrite && this.sharingWritten) {
				throw new Error('Read lost');
			}
			return { type: 'sharingPolicy', policy: this.sharing };
		} else if (command.type === 'systemPolicy') {
			return { type: 'systemPolicy', policy: this.system, support: this.support };
		} else if (command.type === 'computerName') {
			return { type: 'computerName', chosen: this.computerName, system: 'workstation' };
		} else if (command.type === 'setComputerName') {
			this.computerName = command.name;
		} else if (command.type === 'savedRelays') {
			return { type: 'relays', relays: this.relays };
		} else if (command.type === 'setPowerPolicy') {
			this.power = command.policy;
		} else if (command.type === 'setTerminalPolicy') {
			this.terminal = command.policy;
		} else if (command.type === 'setSharingPolicy') {
			this.sharing = { ...this.sharing, ...command.policy };
			this.sharingWritten = true;
		} else if (command.type === 'setSystemPolicy') {
			this.system = command.policy;
		} else if (command.type === 'saveCustomRelays') {
			this.relays = command.relays.map(({ url, authenticated }) => ({ url, authenticated }));
		} else if (command.type === 'setRelayPublic') {
			this.relays = [];
		}
		return { type: 'done' };
	};
}

suite('EngineSettings', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let policies: EnginePolicies;
	let engine: TestEngineMainService;

	/** The settings over the engine's policies, read once as main reads them at startup. */
	async function createSettings(): Promise<EngineSettings> {
		const settings = disposables.add(new EngineSettings(engine));
		await settings.refresh();
		return settings;
	}

	/** The commands of `type` the engine was asked to run. */
	function sent<T extends EngineCommand['type']>(type: T): Extract<EngineCommand, { type: T }>[] {
		return engine.commands.filter((command): command is Extract<EngineCommand, { type: T }> => command.type === type);
	}
	setup(() => {
		policies = new EnginePolicies();
		engine = disposables.add(new TestEngineMainService(policies.handler));
	});
	suite('reading and writing', () => {
		test('policies are read whole, in the units settings show', async () => {
			const settings = await createSettings();
			assert.strictEqual(settings.values['power.incoming.idleTimeout'], 1.5);
			assert.strictEqual(settings.values['terminal.historySize'], 1024);
			assert.deepStrictEqual(settings.sourceErrors, {});
		});
		test('a change is written in the engine\'s units and keeps the policy\'s other fields', async () => {
			const settings = await createSettings();
			await settings.update('power.outgoing.idleTimeout', 2.5);
			assert.strictEqual(policies.power.outgoing_idle_secs, 150);
			assert.strictEqual(policies.power.incoming_idle_secs, 90);
			await settings.update('terminal.replaySize', 16);
			assert.strictEqual(policies.terminal.replay_bytes, 16384);
		});
		test('a terminal limit below the per-device limit is refused before it reaches the engine', async () => {
			const settings = await createSettings();
			await assert.rejects(settings.update('terminal.maxSessions', 4), /Per computer can’t be more than Terminals at once/);
			assert.strictEqual(policies.terminal.max_sessions, 16);
			assert.deepStrictEqual(sent('setTerminalPolicy'), []);
		});
		test('terminal limits that agree are written, and a replay larger than the history is refused', async () => {
			const settings = await createSettings();
			await settings.update('terminal.maxSessionsPerDevice', 4);
			await settings.update('terminal.maxSessions', 4);
			await assert.rejects(settings.update('terminal.replaySize', 2048), /Replay can’t be larger than the history limit/);
			assert.deepStrictEqual([policies.terminal.max_sessions, policies.terminal.max_sessions_per_peer, policies.terminal.replay_bytes], [4, 4, 8192]);
		});
		test('sharing the screen grants viewing, control and the screen\'s sound, and nothing else', async () => {
			const settings = await createSettings();
			await settings.update('sharing.screen', true);
			assert.deepStrictEqual(sent('setSharingPolicy').at(-1)?.policy, { view: true, control: true, system_audio: true });
			assert.strictEqual(policies.sharing.clipboard, true);
			assert.strictEqual(settings.values['sharing.screen'], true);
		});
	});
	suite('computer name', () => {
		test('no chosen name reads as empty, which uses the system\'s', async () => {
			const settings = await createSettings();
			assert.strictEqual(settings.values['application.computerName'], '');
		});
		test('a name is saved, and an empty one goes back to the system\'s', async () => {
			const settings = await createSettings();
			await settings.update('application.computerName', 'Studio Mac');
			assert.strictEqual(settings.values['application.computerName'], 'Studio Mac');
			await settings.update('application.computerName', '');
			assert.deepStrictEqual(sent('setComputerName').map(command => command.name), ['Studio Mac', null]);
		});
	});
	suite('security', () => {
		test('actions this computer does not support are restricted', async () => {
			const settings = await createSettings();
			assert.deepStrictEqual(settings.restricted, ['security.allowRestart']);
		});
		test('a restricted action cannot be enabled', async () => {
			const settings = await createSettings();
			await assert.rejects(settings.update('security.allowRestart', true), /not supported/);
			assert.deepStrictEqual(sent('setSystemPolicy'), []);
		});
		test('a supported action is enabled and the other rules are kept', async () => {
			const settings = await createSettings();
			await settings.update('security.allowShutdown', true);
			assert.deepStrictEqual(policies.system, { allow_lock: true, allow_restart: false, allow_shutdown: true, allow_privacy: false });
		});
	});
	suite('relays', () => {
		test('an authenticated relay without a token is refused before it reaches the engine', async () => {
			const settings = await createSettings();
			await assert.rejects(settings.update('connection.relays', [relay('https://relay.example/', true)]), /token/);
			assert.deepStrictEqual(sent('saveCustomRelays'), []);
		});
		test('a relay token reaches the engine and is never published', async () => {
			const settings = await createSettings();
			await settings.update('connection.relays', [relay('https://relay.example/', true, 'secret')]);
			assert.strictEqual(sent('saveCustomRelays')[0].relays[0].replacementToken, 'secret');
			assert.ok(!JSON.stringify(settings.values).includes('secret'));
		});
		test('a relay saved with a token keeps it when saved again without one', async () => {
			const settings = await createSettings();
			await settings.update('connection.relays', [relay('https://relay.example/', true, 'secret')]);
			await settings.update('connection.relays', [{ url: 'https://relay.example/', authenticated: true }]);
			assert.strictEqual(sent('saveCustomRelays').at(-1)?.relays[0].replacementToken, '');
		});
		test('an empty relay list returns to the public relays', async () => {
			const settings = await createSettings();
			await settings.update('connection.relays', [relay('https://relay.example/')]);
			await settings.update('connection.relays', []);
			assert.deepStrictEqual(engine.commands.slice(-2), [{ type: 'setRelayPublic' }, { type: 'savedRelays' }]);
		});
		test('every relay change written reads restart state again; a refused one does not', async () => {
			const settings = await createSettings();
			await assert.rejects(settings.update('connection.relays', [relay('https://relay.example/', true)]));
			await settings.update('connection.relays', [relay('https://relay.example/', true, 'secret')]);
			await settings.update('connection.relays', [{ url: 'https://relay.example/', authenticated: true }]);
			await settings.update('connection.relays', []);
			assert.strictEqual(engine.restartRefreshes, 3);
		});
	});
	suite('failures', () => {
		test('a refused write publishes what the engine still holds', async () => {
			const settings = await createSettings();
			policies.failWrite = true;
			await assert.rejects(settings.update('sharing.files', true), /could not be changed/);
			assert.strictEqual(settings.values['sharing.files'], false);
		});
		test('a write whose read-back fails publishes no values for its policy, and says why', async () => {
			const settings = await createSettings();
			policies.loseReadAfterWrite = true;
			await assert.rejects(settings.update('sharing.files', true), /current state could not be read/);
			assert.strictEqual('sharing.files' in settings.values, false);
			assert.match(settings.sourceErrors.sharing ?? '', /could not be loaded/);
		});
		test('a stopped engine publishes no values, and says why', async () => {
			const settings = await createSettings();
			engine.setAvailable(false);
			await settings.refresh();
			assert.deepStrictEqual(settings.values, {});
			assert.match(settings.sourceErrors.power ?? '', /unavailable/);
		});
		test('a change while the engine is stopped is refused', async () => {
			const settings = await createSettings();
			engine.setAvailable(false);
			await assert.rejects(settings.update('power.lockAfterControl', true), /unavailable/);
			assert.deepStrictEqual(sent('setPowerPolicy'), []);
		});
		test('a restarted engine is read again', async () => {
			const settings = await createSettings();
			engine.setAvailable(false);
			await settings.refresh();
			engine.setAvailable(true);
			await timeout(0);
			assert.strictEqual(settings.values['power.lockAfterControl'], false);
		});
	});
});

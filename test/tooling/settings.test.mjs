// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeEngine } from './fixtures.mjs';
import { build } from 'esbuild';

const output = await build({
	stdin: {
		contents: `export { EngineSettings } from './src/platform/configuration/electron-main/engineSettings.js';`,
		resolveDir: process.cwd(), loader: 'ts',
	}, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json',
});

const { EngineSettings } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

function relay(url, authenticated = false, replacementToken = '') {
	return { url, authenticated, replacementToken };
}

/** An engine holding each policy, with switches to refuse writes or reads. */
function policyEngine() {
	const state = {
		power: {
			incoming_display: false, incoming_system: true, outgoing_display: false, outgoing_system: false,
			incoming_idle_secs: 90, outgoing_idle_secs: 0, lock_after_control: false,
		},
		terminal: {
			allow_persistence: true, max_sessions: 16, max_sessions_per_peer: 16, history_bytes: 1048576, replay_bytes: 8192, detached_timeout_secs: 300,
		},
		sharing: {
			view: false, control: false, clipboard: true, files: false, terminal: false,
			camera: false, system_audio: false, microphone: false, recording: false, system_actions: false,
		},
		system: { allow_lock: true, allow_restart: false, allow_shutdown: false, allow_privacy: false },
		support: { lock: true, restart: false, shutdown: true, privacy: true },
		relays: [],
		failWrite: false, failRead: false, restartRefreshes: 0, commands: [],
	};
	const engine = fakeEngine({
		async engineCommand(command) {
			state.commands.push(command);
			if (command.type.startsWith('set') || command.type === 'saveCustomRelays') {
				if (state.failWrite) {
					throw new Error('Engine refused');
				}
			} else if (state.failRead) {
				throw new Error('Engine stopped');
			}
			switch (command.type) {
				case 'powerPolicy': return { type: 'powerPolicy', policy: state.power };
				case 'terminalPolicy': return { type: 'terminalPolicy', policy: state.terminal };
				case 'sharingPolicy': return { type: 'sharingPolicy', policy: state.sharing };
				case 'systemPolicy': return { type: 'systemPolicy', policy: state.system, support: state.support };
				case 'savedRelays': return { type: 'relays', relays: state.relays };
				case 'setPowerPolicy': state.power = command.policy; break;
				case 'setTerminalPolicy': state.terminal = command.policy; break;
				case 'setSharingPolicy': state.sharing = { ...state.sharing, ...command.policy }; break;
				case 'setSystemPolicy': state.system = command.policy; break;
				case 'saveCustomRelays': state.relays = command.relays.map(({ url, authenticated }) => ({ url, authenticated })); break;
				case 'setRelayPublic': state.relays = []; break;
			}
			return { type: 'done' };
		},
	});
	engine.refreshRestartRequired = async () => {
		state.restartRefreshes++;
	};
	return { engine, state };
}

test('engine policies read whole and write through a read-modify-write with read-back', async () => {
	const { engine, state } = policyEngine();
	const settings = new EngineSettings(engine);
	try {
		await settings.refresh();
		assert.equal(settings.values['power.incoming.idleTimeout'], 1.5);
		assert.equal(settings.values['terminal.historySize'], 1024);
		assert.deepEqual(settings.sourceErrors, {});
		await settings.update('power.outgoing.idleTimeout', 2.5);
		assert.equal(state.power.outgoing_idle_secs, 150);
		assert.equal(state.power.incoming_idle_secs, 90, 'Another field of the policy changed');
		await settings.update('terminal.replaySize', 16);
		assert.equal(state.terminal.replay_bytes, 16384);
		await assert.rejects(settings.update('terminal.maxSessions', 4), /Per computer can’t be more than Terminals at once/);
		assert.equal(state.terminal.max_sessions, 16, 'A limit below the per-device limit reached the engine');
		await settings.update('terminal.maxSessionsPerDevice', 4);
		await settings.update('terminal.maxSessions', 4);
		await assert.rejects(settings.update('terminal.replaySize', 2048), /Replay can’t be larger than the history limit/);
		assert.deepEqual([state.terminal.max_sessions, state.terminal.max_sessions_per_peer], [4, 4]);
		await settings.update('sharing.screen', true);
		assert.deepEqual(state.commands.filter(command => command.type === 'setSharingPolicy').at(-1).policy, { view: true, control: true, system_audio: true }, 'Screen sharing did not bring its sound');
		assert.equal(state.sharing.clipboard, true, 'Sharing write replaced unrelated consent');
		assert.equal(settings.values['sharing.screen'], true);
	} finally {
		settings.dispose();
	}
});

test('unsupported actions are restricted and cannot be enabled; supported ones keep the other rules', async () => {
	const { engine, state } = policyEngine();
	const settings = new EngineSettings(engine);
	try {
		await settings.refresh();
		assert.deepEqual(settings.restricted, ['security.allowRestart']);
		await assert.rejects(settings.update('security.allowRestart', true), /not supported/);
		assert.ok(!state.commands.some(command => command.type === 'setSystemPolicy'));
		await settings.update('security.allowShutdown', true);
		assert.deepEqual(state.system, { allow_lock: true, allow_restart: false, allow_shutdown: true, allow_privacy: false });
	} finally {
		settings.dispose();
	}
});

test('relay credentials are write-only, saved tokens are kept for their URL, and relay changes refresh restart state', async () => {
	const { engine, state } = policyEngine();
	const settings = new EngineSettings(engine);
	try {
		await settings.refresh();
		await assert.rejects(settings.update('connection.relays', [relay('https://relay.example/', true)]), /token/);
		assert.ok(!state.commands.some(command => command.type === 'saveCustomRelays'), 'An invalid relay list reached the engine');
		await settings.update('connection.relays', [relay('https://relay.example/', true, 'secret')]);
		assert.equal(state.commands.find(command => command.type === 'saveCustomRelays').relays[0].replacementToken, 'secret');
		assert.ok(!JSON.stringify(settings.values).includes('secret'), 'A saved token was published');
		await settings.update('connection.relays', [{ url: 'https://relay.example/', authenticated: true }]);
		assert.equal(state.commands.filter(command => command.type === 'saveCustomRelays').at(-1).relays[0].replacementToken, '');
		await settings.update('connection.relays', []);
		assert.equal(state.commands.at(-2).type, 'setRelayPublic');
		assert.equal(state.restartRefreshes, 3);
	} finally {
		settings.dispose();
	}
});

test('failed writes publish what the engine holds; failed read-backs and stopped engines publish no values', async () => {
	const { engine, state } = policyEngine();
	const settings = new EngineSettings(engine);
	try {
		await settings.refresh();
		state.failWrite = true;
		await assert.rejects(settings.update('sharing.files', true), /could not be changed/);
		assert.equal(settings.values['sharing.files'], false);
		state.failWrite = false;
		const original = engine.execute;
		let written = false;
		engine.execute = async command => {
			if (command.type === 'sharingPolicy' && written) {
				throw new Error('Read lost');
			}
			written ||= command.type === 'setSharingPolicy';
			return original(command);
		};
		await assert.rejects(settings.update('sharing.files', true), /current state could not be read/);
		assert.equal('sharing.files' in settings.values, false);
		assert.match(settings.sourceErrors.sharing, /could not be loaded/);
		engine.execute = original;
		engine.setAvailable(false);
		await settings.refresh();
		assert.deepEqual(settings.values, {});
		assert.match(settings.sourceErrors.power, /unavailable/);
		await assert.rejects(settings.update('power.lockAfterControl', true), /unavailable/);
		engine.setAvailable(true);
		await new Promise(resolve => setTimeout(resolve, 0));
		assert.equal(settings.values['power.lockAfterControl'], false, 'A restarted engine was not read again');
	} finally {
		settings.dispose();
	}
});

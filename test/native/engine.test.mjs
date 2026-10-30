// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, parse, relative, sep } from 'node:path';
import { createRequire } from 'node:module';
import test from 'node:test';

const addon = createRequire(import.meta.url)('../../out/native/vnilink.node');

const execute = async (engine, command) => JSON.parse(await engine.execute(JSON.stringify(command)));

async function waitFor(condition, name) {
	const deadline = Date.now() + 20000;
	while (!await condition()) {
		assert(Date.now() < deadline, `${name} timed out`);
		await new Promise(resolve => setTimeout(resolve, 50));
	}
}

test('two Rust engines pair through a signed file, retain grants, and reject commands after shutdown', { timeout: 30000 }, async () => {
	const first = await addon.openTemporaryEngine();
	let second;
	try {
		second = await addon.openTemporaryEngine();
		for (const codec of ['auto', 'h264', 'h265', 'vp8', 'vp9', 'av1']) {
			assert.equal((await execute(first, { type: 'setDesktopCodecDefault', codec })).type, 'done');
		}
		await assert.rejects(execute(first, { type: 'setDesktopCodecDefault', codec: 'invalid' }), /InvalidSetting/);
		await assert.rejects(execute(first, { type: 'setDesktopCodecDefault', codec: 'auto', extra: true }));
		await assert.rejects(execute(first, { type: 'unsupportedCommand' }));
		// The diagnostic snapshot crosses as the engine's own bounded JSON, schema version 5.
		const diagnostics = await execute(first, { type: 'diagnosticSnapshot' });
		assert.equal(diagnostics.type, 'diagnosticSnapshot');
		assert.ok(Buffer.byteLength(diagnostics.snapshot) <= 64 * 1024);
		const parsed = JSON.parse(diagnostics.snapshot);
		assert.equal(parsed.version, 5);
		assert.equal(parsed.last_end, null);
		assert.ok(!diagnostics.snapshot.includes(JSON.parse(await first.execute(JSON.stringify({ type: 'endpoint' }))).endpointId));
		await assert.rejects(execute(first, { type: 'diagnosticSnapshot', extra: true }));
		const localSecurity = await execute(first, { type: 'systemPolicy' });
		assert.equal(localSecurity.type, 'systemPolicy');
		assert.equal(typeof localSecurity.support.lock, 'boolean');
		await execute(first, { type: 'setSystemPolicy', policy: { ...localSecurity.policy, allow_lock: true } });
		assert.equal((await execute(first, { type: 'systemPolicy' })).policy.allow_lock, true);
		await assert.rejects(execute(first, { type: 'setSystemPolicy', policy: { ...localSecurity.policy, unknown: true } }));
		const input = { mode: 'Map', swap: true, pointer: { sensitivity_percent: 175, reverse_wheel: true, swap_buttons: true } };
		assert.equal((await execute(first, { type: 'setInputDefaults', ...input })).type, 'done');
		assert.equal((await execute(first, { type: 'keyboardMode' })).mode, 'Map');
		assert.equal((await execute(first, { type: 'swapControlAndCommand' })).enabled, true);
		await assert.rejects(execute(first, { type: 'setInputDefaults', ...input, pointer: { ...input.pointer, sensitivity_percent: 0 } }));
		// Desktop input goes on a desktop's data channel, never as a command.
		for (const command of [
			{ type: 'submitKey', sessionId: '1', generation: '1', usage: 4, down: true },
			{ type: 'submitText', sessionId: '1', generation: '1', text: 'a' },
			{ type: 'submitLocalKey', sessionId: '1', generation: '1', usage: 4, down: true, text: 'a', composing: false },
			{ type: 'releaseSessionInput', sessionId: '1', generation: '1' },
		]) {
			await assert.rejects(execute(first, command), /Desktop input goes on its data channel/, command.type);
		}
		// The transfer folder is only named here; the first transfer creates it.
		const transfers = join(tmpdir(), `vnilink-transfers-${String(process.pid)}`);
		assert.equal((await execute(first, { type: 'setTransferDirectory', path: transfers })).type, 'done');
		assert(!existsSync(transfers), 'Naming the transfer folder created it');
		await assert.rejects(execute(first, { type: 'setTransferDirectory', path: 'relative/Transfers' }), /InvalidSetting/);
		await assert.rejects(execute(first, { type: 'setTransferDirectory', path: '/does-not-exist/vnilink-test/Transfers' }), /InvalidSetting/);
		await assert.rejects(execute(first, { type: 'stopHostedPeer', sessionId: 'not-a-number' }), /Invalid session ID/);
		await assert.rejects(execute(first, { type: 'stopHostedPeer', sessionId: '1', extra: true }));
		await assert.rejects(execute(first, { type: 'stopHostedPeer', sessionId: '1' }), /NoSession/);
		// The host's own user chooses among detected shells; no command takes a program path.
		const shells = await execute(first, { type: 'terminalShells' });
		assert.equal(shells.type, 'terminalShells');
		assert(shells.shells.some(shell => shell.id === shells.automatic), 'Automatic is not a detected shell');
		assert.equal(shells.current, null, 'A temporary engine offered a shell before one was chosen');
		await assert.rejects(execute(first, { type: 'setTerminalShell', shell: '/tmp/not-a-shell' }), /InvalidSetting/);
		await assert.rejects(execute(first, { type: 'setTerminalShell', shell: shells.automatic, program: '/bin/sh' }));
		await assert.rejects(execute(first, { type: 'enableNativeTerminal', program: '/bin/sh', directory: '/tmp' }));
		await execute(first, { type: 'setTerminalShell', shell: null });
		assert.equal((await execute(first, { type: 'terminalShells' })).current, shells.automatic);
		const runtime = await execute(first, { type: 'nativeRuntime' });
		assert.equal(runtime.type, 'nativeRuntime');
		assert.equal(runtime.capabilities.length, 13);
		assert.equal(runtime.backends.length, 0);
		assert.equal(runtime.capabilities.find(cap => cap.name === 'camera').available, false);
		assert.equal((await execute(first, { type: 'setNativeCameraSources', ids: [] })).type, 'done');
		assert.equal((await execute(first, { type: 'setNativeAudioSources', microphones: [], system: [] })).type, 'done');
		const power = await execute(first, { type: 'powerPolicy' });
		assert.equal(power.type, 'powerPolicy');
		assert.equal((await execute(first, { type: 'setPowerPolicy', policy: power.policy })).type, 'done');
		await assert.rejects(execute(first, { type: 'setPowerPolicy', policy: { ...power.policy, outgoing_idle_secs: 86401 } }));
		assert.equal((await execute(first, { type: 'powerStatus' })).type, 'powerStatus');
		await assert.rejects(execute(first, { type: 'nativeRuntime', unknown: true }));
		await assert.rejects(execute(first, { type: 'recordingStatus', id: '1' }));
		await assert.rejects(execute(first, { type: 'stopRecording', id: '1' }));
		await assert.rejects(execute(first, { type: 'releaseRecording', id: '1' }));
		// A camera recording is accepted as a command and needs a camera open on that connection.
		const recording = { type: 'startRecording', sessionId: '1', generation: '1', destination: '/tmp/vnilink-camera.mkv', maxBytes: '1000000', maxDurationMs: '60000' };
		await assert.rejects(execute(first, { ...recording, source: 'camera' }), /NoSession: no camera is open/);
		await assert.rejects(execute(first, { ...recording, source: 'microphone' }), /Invalid|unknown variant/);
		// Locking or restarting another computer needs a connection to it; nothing counts down here yet.
		await assert.rejects(execute(first, { type: 'systemAction', sessionId: '1', generation: '1', action: 'lock' }), /NoSession|StaleSession/);
		await assert.rejects(execute(first, { type: 'systemAction', sessionId: '1', generation: '1', action: 'format' }), /Invalid system action/);
		await assert.rejects(execute(first, { type: 'systemAction', sessionId: '1', generation: '1', action: 'lock', force: true }));
		// Privacy mode is asked of a connected computer too, and takes a plain switch.
		await assert.rejects(execute(first, { type: 'setSystemPrivacy', sessionId: '1', generation: '1', enabled: true }), /NoSession|StaleSession/);
		await assert.rejects(execute(first, { type: 'systemPrivacy', sessionId: '1', generation: '1' }), /NoSession|StaleSession/);
		await assert.rejects(execute(first, { type: 'setSystemPrivacy', sessionId: '1', generation: '1', enabled: 'yes' }));
		await assert.rejects(execute(first, { type: 'setSystemPrivacy', sessionId: '1', generation: '1', enabled: true, inputBlocked: false }));
		assert.deepEqual(await execute(first, { type: 'scheduledSystemActions' }), { type: 'scheduledSystemActions', actions: [] });
		await assert.rejects(execute(first, { type: 'cancelHostedSystemAction', id: '1' }), /InvalidSetting/);
		await assert.rejects(execute(first, { type: 'runHostedSystemActionNow', id: '1' }), /InvalidSetting/);
		await assert.rejects(async () => first.openDataChannel('camera', '1', '1', 24, 80));
		await assert.rejects(async () => first.openDataChannel('audio', '1', '1', 24, 80, JSON.stringify({ type: 'camera', deviceId: 'camera', width: 1280, height: 720, fps: 30 })));
		await assert.rejects(async () => first.openDataChannel('camera', '1', '1', 24, 80, JSON.stringify({ type: 'camera', deviceId: 'camera', width: 1280, height: 720, fps: 30, endpointId: 'other' })));
		const quality = { max_bitrate_kbps: 2000, max_frame_rate: 30, max_width: 1280, max_height: 720 };
		assert.equal((await execute(first, { type: 'relayRestartRequired' })).required, false);
		assert.equal((await execute(first, { type: 'setDesktopQualityDefaults', quality })).type, 'done');
		await assert.rejects(execute(first, { type: 'setDesktopQualityDefaults', quality: { ...quality, max_frame_rate: 61 } }));
		// A session carries no picture: what a desktop session once took is no command now.
		for (const removed of [
			{ type: 'connect', endpointId: 'peer' },
			{ type: 'beginConnect', endpointId: 'peer' },
			{ type: 'updateSessionQuality', sessionId: '1', generation: '1', role: 'Controller', quality },
			{ type: 'subscribeVideo' },
			{ type: 'subscribeSessionVideo', sessionId: '1', generation: '1' },
			{ type: 'pollVideo', subscriptionId: '1' },
			{ type: 'releaseVideo', subscriptionId: '1' },
			{ type: 'publishVideo', codec: 'h264', sessionId: '1', keyframe: true, timestampUs: '0', data: 'AQ==' },
		]) {
			await assert.rejects(execute(first, removed), /Invalid|unknown variant/, `${removed.type} is refused`);
		}
		// A display selection is Desktop input: only its data channel takes it.
		await assert.rejects(execute(first, { type: 'selectDisplays', sessionId: '1', generation: '1', ids: ['1'] }), /data channel/);
		const a = (await execute(first, { type: 'snapshot' })).snapshot;
		const b = (await execute(second, { type: 'snapshot' })).snapshot;
		assert.notEqual(a.endpoint_id, b.endpoint_id);
		assert.equal(a.hosting, false);
		assert.equal(b.devices.length, 0);
		const { file } = await execute(first, { type: 'exportPairingFile', ttlSeconds: 60, allowControl: false });
		assert.match(file.check, /^[0-9]{3}$/);
		const { preview } = await execute(second, { type: 'openPairingFile', bytes: file.bytes });
		assert.equal(preview.endpoint_id, a.endpoint_id);
		assert.equal(typeof preview.id, 'string');
		const { device } = await execute(second, { type: 'confirmPairing', previewId: preview.id, digits: file.check });
		assert.equal(device.endpoint_id, a.endpoint_id);
		assert.deepEqual(device.grants, { view: false, control: false, clipboard: false, transfer: false });
		await assert.rejects(async () => second.execute(JSON.stringify({ type: 'snapshot', unknown: true })));
		await assert.rejects(async () => second.execute(JSON.stringify({ type: 'publishVideo' })));
		await execute(second, { type: 'setPeerName', endpointId: a.endpoint_id, name: 'My computer' });
		assert.equal((await execute(second, { type: 'snapshot' })).snapshot.devices[0].pet_name, 'My computer');
		const policy = (await execute(second, { type: 'sharingPolicy' })).policy;
		assert(Object.values(policy).every(value => value === false));
		await execute(second, { type: 'setSharingPolicy', policy: { view: true } });
		await execute(second, { type: 'setSharingPolicy', policy: { terminal: true } });
		assert.equal((await execute(second, { type: 'sharingPolicy' })).policy.view, true);
		assert.equal((await execute(second, { type: 'sharingPolicy' })).policy.terminal, true);
		for (const policy of [{}, { files: null }, { files: 'yes' }, { files: true, extra: false }]) {
			await assert.rejects(execute(second, { type: 'setSharingPolicy', policy }));
		}
		assert.equal(device.os, process.platform === 'darwin' ? 'macos' : process.platform === 'win32' ? 'windows' : 'linux');
		assert.equal(device.presence, 'disconnected');
		// Presence follows reachability without any connect call. The paired computer is
		// reachable but not hosting; once it hosts, it still refuses this side, which only it
		// has not approved yet (the control request does that), so this side shows it offline.
		const presence = async () => (await execute(second, { type: 'snapshot' })).snapshot.devices[0].presence;
		await waitFor(async () => await presence() === 'notAccepting', 'not accepting');
		assert.equal((await execute(second, { type: 'peerAvailability', endpointId: a.endpoint_id })).report, null);
		await execute(first, { type: 'startHosting' });
		await execute(second, { type: 'beginConnectServices', endpointId: a.endpoint_id }).catch(() => undefined);
		await waitFor(async () => await presence() === 'disconnected', 'refused while unapproved');
		assert.equal((await execute(first, { type: 'hostedServices' })).services.length, 0);
		assert.deepEqual((await execute(second, { type: 'snapshot' })).snapshot.devices[0].grants, { view: true, control: false, clipboard: false, transfer: false });
		await execute(second, { type: 'unpair', endpointId: a.endpoint_id });
		assert.equal((await execute(second, { type: 'snapshot' })).snapshot.devices.length, 0);
	} finally {
		await execute(first, { type: 'shutdown' });
		if (second) {
			await execute(second, { type: 'shutdown' });
		}
	}
	await assert.rejects(execute(first, { type: 'snapshot' }), /shut down/);
});

test('this computer\'s disk lists, makes, renames and deletes through its own engine', { timeout: 30000 }, async () => {
	const engine = await addon.openTemporaryEngine();
	// The native form: on Windows it also spells out short (8.3) names, as the
	// engine does.
	const folder = realpathSync.native(mkdtempSync(join(tmpdir(), 'vnilink-local-')));
	try {
		writeFileSync(join(folder, 'note.txt'), 'note');
		assert.deepEqual((await execute(engine, { type: 'localPlaces' })).places.roots, [], 'a temporary engine shares nothing');
		await execute(engine, { type: 'enableFilesystemAccess' });
		const { places } = await execute(engine, { type: 'localPlaces' });
		const drive = parse(folder).root;
		const root = places.roots.find(candidate => drive === '/' ? candidate.name === '/' : drive.toUpperCase().startsWith(candidate.name));
		assert(root, `no root holds ${folder}`);
		const path = relative(drive, folder).split(sep).join('/');
		const list = async () => (await execute(engine, { type: 'listLocal', root: root.id, path, offset: '0' })).page;
		const page = await list();
		assert.deepEqual(page.entries.map(entry => [entry.name, entry.kind, entry.size]), [['note.txt', 'file', '4']]);
		// A file dropped from Finder or Explorer is named as the panes name it.
		assert.deepEqual((await execute(engine, { type: 'locateLocal', path: join(folder, 'note.txt') })).location, { root: root.id, path, name: 'note.txt' });
		assert.equal((await execute(engine, { type: 'createLocalDir', root: root.id, path, name: 'made' })).type, 'done');
		await assert.rejects(execute(engine, { type: 'createLocalDir', root: root.id, path, name: 'made' }), /FileExists/);
		const made = (await list()).entries.find(entry => entry.name === 'made');
		assert.equal((await execute(engine, { type: 'renameLocal', root: root.id, path, name: 'made', token: made.token, newName: 'renamed' })).type, 'done');
		assert(existsSync(join(folder, 'renamed')));
		assert.equal((await execute(engine, { type: 'deleteLocal', root: root.id, path, name: 'renamed', token: null, trash: false })).type, 'done');
		assert(!existsSync(join(folder, 'renamed')));
		// A change made outside VniLink is said as an event for the folder's watch.
		const { subscriptionId } = await execute(engine, { type: 'subscribeEvents' });
		const { watch } = await execute(engine, { type: 'watchLocal', root: root.id, path });
		writeFileSync(join(folder, 'outside.txt'), 'outside');
		let heard = false;
		for (let attempt = 0; attempt < 50 && !heard; attempt++) {
			await new Promise(resolve => setTimeout(resolve, 100));
			const { events } = await execute(engine, { type: 'pollEvents', subscriptionId, maxEvents: 64 });
			heard = events.some(event => event.type === 'folderChanged' && event.watch === watch);
		}
		assert(heard, 'a file written outside VniLink was not reported');
		assert.equal((await execute(engine, { type: 'unwatchLocal', watch })).type, 'done');
	} finally {
		await execute(engine, { type: 'shutdown' });
		rmSync(folder, { recursive: true, force: true });
	}
});

import assert from 'node:assert/strict';
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
		await assert.rejects(execute(first, { type: 'updateSessionQuality', sessionId: '1', generation: '1', role: 'Controller', quality }), /NoSession/);
		await assert.rejects(execute(first, { type: 'updateSessionQuality', sessionId: '1', role: 'Controller', quality }));
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

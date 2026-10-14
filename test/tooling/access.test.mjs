import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const result = await build({ stdin: { contents: `export { AccessService } from './src/workbench/services/access/electron-browser/accessService.js'; export { AccessReadbackError } from './src/workbench/services/access/common/accessService.js'; export { isEngineCommand, isEngineReply } from './src/platform/engine/common/engine.js';`, loader: 'ts', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node', tsconfig: 'tsconfig.json' });

const { AccessService, AccessReadbackError, isEngineCommand, isEngineReply } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

function fixture() {
	const devices = [{ endpoint_id: 'one', pet_name: 'Work', hostname: null, grants: { view: true, control: false, clipboard: false, transfer: true } }, { endpoint_id: 'two', pet_name: null, hostname: 'Other', grants: { view: false, control: false, clipboard: false, transfer: false } }];
	const permissions = { desktop: true, files: true, terminal: false, camera: false, system_audio: false, microphone: false, recording: false, system_actions: false };
	let policy = { allow_lock: true, allow_restart: false, allow_shutdown: false, allow_privacy: false };
	const support = { lock: true, restart: false, shutdown: false, privacy: false };
	const calls = [];
	const api = {
		async engineCommand(command) {
			calls.push(command);
			if (command.type === 'snapshot') {
				return { type: 'snapshot', snapshot: { devices } };
			}
			if (command.type === 'servicePermissions') {
				return { type: 'servicePermissions', permissions: { ...permissions } };
			}
			if (command.type === 'systemPolicy') {
				return { type: 'systemPolicy', policy: { ...policy }, support };
			}
			if (command.type === 'setSystemPolicy') {
				policy = { ...command.policy };
			}
			if (command.type === 'authorize') {
				const { view, control, clipboard, transfer } = command;
				devices.find(device => device.endpoint_id === command.endpointId).grants = { view, control, clipboard, transfer };
			}
			if (command.type === 'authorizeRecording') {
				permissions.recording = command.allowed;
			}
			if (command.type === 'authorizeService') {
				const keys = { Desktop: 'desktop', Files: 'files', Terminal: 'terminal', Camera: 'camera', SystemAudio: 'system_audio', Microphone: 'microphone', System: 'system_actions' };
				permissions[keys[command.kind]] = command.allowed;
			}
			return { type: 'done' };
		}
	};
	return { devices, calls, api };
}

test('grant writes target the selected identity and preserve unrelated grants', async () => {
	const { api, calls, devices } = fixture();
	const service = new AccessService(api, true);
	const peers = await service.listDevices();
	assert.deepEqual(peers, [{ id: 'one', name: 'Work' }, { id: 'two', name: 'Other' }]);
	await Promise.all([service.setPermission('one', 'control', true), service.setPermission('one', 'clipboard', true)]);
	assert.deepEqual(devices[0].grants, { view: true, control: true, clipboard: true, transfer: true });
	assert.deepEqual(devices[1].grants, { view: false, control: false, clipboard: false, transfer: false });
	assert.ok(calls.filter(call => call.type === 'authorize').every(call => call.endpointId === 'one'));
	assert.equal((await service.setPermission('one', 'recording', true)).permissions.recording, true);
	assert.equal((await service.setPermission('one', 'terminal', true)).permissions.terminal, true);
});

test('unknown peers and unavailable engines never reach a permission write', async () => {
	const { api, calls } = fixture();
	await assert.rejects(new AccessService(api, true).setPermission('missing', 'view', true));
	assert.ok(!calls.some(call => call.type === 'authorizeService'));
	calls.length = 0;
	await assert.rejects(new AccessService(api, false).loadPolicy());
	assert.equal(calls.length, 0);
});

test('local policy updates preserve other rules and report actual backend support', async () => {
	const { api } = fixture();
	const service = new AccessService(api, true);
	const state = await service.setPolicy('allow_lock', false);
	assert.deepEqual(state.policy, { allow_lock: false, allow_restart: false, allow_shutdown: false, allow_privacy: false });
	assert.equal(state.support.restart, false);
	assert.equal(isEngineCommand({ type: 'setSystemPolicy', policy: state.policy }), true);
	assert.equal(isEngineCommand({ type: 'setSystemPolicy', policy: { ...state.policy, extra: true } }), false);
	assert.equal(isEngineReply({ type: 'systemPolicy', ...state }), true);
	assert.equal(isEngineReply({ type: 'systemPolicy', policy: state.policy, support: { lock: 'yes' } }), false);
});

test('read-back failure after a committed permission write is distinguishable', async () => {
	const { api } = fixture();
	const original = api.engineCommand;
	let committed = false;
	api.engineCommand = async command => {
		if (committed) {
			throw new Error('read failed');
		}
		const reply = await original(command);
		if (command.type === 'authorizeRecording') {
			committed = true;
		}
		return reply;
	};
	await assert.rejects(new AccessService(api, true).setPermission('one', 'recording', true), AccessReadbackError);
});

test('unavailable system actions cannot be enabled through the settings service', async () => {
	const { api, calls } = fixture();
	const service = new AccessService(api, true);
	await assert.rejects(service.setPolicy('allow_shutdown', true), /unavailable/);
	assert(!calls.some(command => command.type === 'setSystemPolicy'));
});

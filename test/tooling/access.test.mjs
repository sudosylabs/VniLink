import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeEngine } from './fixtures.mjs';
import { build } from 'esbuild';

const result = await build({ stdin: { contents: `export { AccessService } from './src/workbench/services/access/electron-browser/accessService.js'; export { AccessReadbackError } from './src/workbench/services/access/common/accessService.js'; export { isEngineCommand, isEngineReply } from './src/platform/engine/common/engine.js';`, loader: 'ts', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node', tsconfig: 'tsconfig.json' });

const { AccessService, AccessReadbackError, isEngineCommand, isEngineReply } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

function fixture() {
	const calls = [];
	let sharing = { view: false, control: false, clipboard: false, files: false, terminal: false, camera: false, system_audio: false, microphone: false, recording: false, system_actions: false };
	let policy = { allow_lock: true, allow_restart: false, allow_shutdown: false, allow_privacy: false };
	const support = { lock: true, restart: false, shutdown: false, privacy: false };
	const api = {
		async engineCommand(command) {
			calls.push(command);
			if (command.type === 'sharingPolicy') {
				return { type: 'sharingPolicy', policy: sharing };
			}
			if (command.type === 'setSharingPolicy') {
				sharing = { ...sharing, ...command.policy };
				return { type: 'done' };
			}
			if (command.type === 'systemPolicy') {
				return { type: 'systemPolicy', policy, support };
			}
			if (command.type === 'setSystemPolicy') {
				policy = command.policy;
				return { type: 'done' };
			}
			throw new Error('Unexpected command ' + command.type);
		}
	};
	return { calls, api };
}

test('global sharing writes require no device and preserve independent switches', async () => {
	const { api, calls } = fixture();
	const service = new AccessService(fakeEngine(api, true));
	await Promise.all([service.setSharing({ terminal: true }), service.setSharing({ camera: true })]);
	const policy = await service.loadSharing();
	assert.equal(policy.terminal, true);
	assert.equal(policy.camera, true);
	assert.equal(policy.files, false);
	assert(calls.every(command => !('endpointId' in command)));
});

test('unavailable engines cannot read or change global sharing', async () => {
	const { api, calls } = fixture();
	const service = new AccessService(fakeEngine(api, false));
	await assert.rejects(service.loadSharing(), /unavailable/);
	await assert.rejects(service.setSharing({ terminal: true }), /unavailable/);
	assert.equal(calls.length, 0);
});

test('local policy updates preserve other rules and report actual backend support', async () => {
	const { api } = fixture();
	const service = new AccessService(fakeEngine(api, true));
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
		if (command.type === 'setSharingPolicy') {
			committed = true;
		}
		return reply;
	};
	await assert.rejects(new AccessService(fakeEngine(api, true)).setSharing({ recording: true }), AccessReadbackError);
});

test('unavailable system actions cannot be enabled through the settings service', async () => {
	const { api, calls } = fixture();
	const service = new AccessService(fakeEngine(api, true));
	await assert.rejects(service.setPolicy('allow_shutdown', true), /unavailable/);
	assert(!calls.some(command => command.type === 'setSystemPolicy'));
});

test('a failure after native revocation reports saved consent instead of restoring stale UI state', async () => {
	const { api } = fixture();
	const original = api.engineCommand;
	api.engineCommand = async command => {
		const reply = await original(command);
		if (command.type === 'setSharingPolicy') {
			throw new Error('native privacy restore failed');
		}
		return reply;
	};
	const service = new AccessService(fakeEngine(api, true));
	await assert.rejects(service.setSharing({ files: true }), error => {
		assert.equal(error.policy.files, true);
		assert.match(error.cause.message, /privacy restore failed/);
		return true;
	});
});

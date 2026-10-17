import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeEngine } from './fixtures.mjs';
import { build } from 'esbuild';

const bundle = await build({ stdin: { contents: `export { synchronizeNativePermissions } from './src/platform/engine/electron-main/nativeRuntime.js'; export { NativeRuntimeService } from './src/workbench/services/nativeRuntime/electron-browser/nativeRuntimeService.js'; export { isEngineCommand, isEngineReply, nativeCapabilityNames } from './src/platform/engine/common/engine.js';`, loader: 'ts', resolveDir: process.cwd() }, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json' });

const { synchronizeNativePermissions, NativeRuntimeService, isEngineCommand, isEngineReply, nativeCapabilityNames } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

const configuration = { snapshot: { values: { 'recording.defaults': { directory: '', maxBytes: '1048576', maxDurationMs: '1000', maxSegments: 1, maxTotalBytes: '1048576' } } } };

const unusedHost = { getCaptureDisplays: () => assert.fail('Unexpected display read'), pickFolder: () => assert.fail('Unexpected folder picker') };

const unusedRecording = { getDefaultDirectory: () => assert.fail('Unexpected folder read'), createDestination: () => assert.fail('Unexpected destination') };

const runtime = { type: 'nativeRuntime', backends: [{ name: 'terminal', ready: true, error: null }], capabilities: nativeCapabilityNames.map(name => ({ name, available: name === 'terminal', summary: `${name}: available` })) };

test('native runtime and device boundaries reject duplicates, unknown permissions and malformed policies', () => {
	assert(isEngineReply(runtime));
	assert(!isEngineReply({ ...runtime, capabilities: runtime.capabilities.map(() => runtime.capabilities[0]) }));
	assert(isEngineCommand({ type: 'nativeRuntime' }));
	assert(!isEngineCommand({ type: 'nativeRuntime', endpointId: 'unexpected' }));
	assert(!isEngineCommand({ type: 'setNativeCameraSources', ids: ['one', 'one'] }));
	assert(!isEngineCommand({ type: 'setNativeAudioSources', microphones: ['one'], system: ['one'] }));
	assert(!isEngineCommand({ type: 'reportNativePermissions', screen: true, input: 'granted', camera: false, microphone: false }));
	assert(!isEngineReply({ type: 'nativeDevices', devices: [{ id: 'one', name: 'Camera', kind: 'unknown' }] }));
});

test('OS permission synchronization does not install input on a Mac without Accessibility consent', async () => {
	const calls = [];
	const engine = {
		available: true, execute: async command => {
			calls.push(command);
			return { type: 'done' };
		}
	};
	const denied = { development: false, permissions: ['screen', 'accessibility', 'camera', 'microphone'].map(kind => ({ kind, status: 'denied', canRequest: false, canOpenSettings: true })) };
	await synchronizeNativePermissions(engine, 'darwin', denied);
	assert.deepEqual(calls.map(c => c.type), ['reportNativePermissions']);
	assert.equal(calls[0].input, false);
	calls.length = 0;
	await synchronizeNativePermissions(engine, 'darwin', { ...denied, permissions: denied.permissions.map(p => ({ ...p, status: p.kind === 'accessibility' ? 'granted' : p.status })) });
	assert.deepEqual(calls.map(c => c.type), ['reportNativePermissions', 'enableNativeInput']);
	assert(!calls.some(c => c.type === 'startHosting' || c.type === 'setNativeCameraSources'));
	calls.length = 0;
	await synchronizeNativePermissions({ ...engine, available: false }, 'darwin', denied);
	assert.equal(calls.length, 0);
});

test('device approvals capture queued drafts and immutable runtime readback; failed backend updates do not report success', async () => {
	const calls = [];
	let fail = false;
	const service = new NativeRuntimeService(fakeEngine({
		engineCommand: async command => {
			calls.push(command);
			if (command.type === 'nativeRuntime') {
				return runtime;
			}
			if (fail) {
				throw new Error('Source unavailable');
			}
			return { type: 'done' };
		}
	}), unusedHost, unusedRecording, configuration);
	const cameras = ['chosen'];
	const writing = service.approveCameras(cameras);
	cameras[0] = 'changed';
	const result = await writing;
	assert.equal(calls[0].ids[0], 'chosen');
	assert(Object.isFrozen(result.capabilities[0]));
	assert(Object.isFrozen(result.backends));
	fail = true;
	await assert.rejects(service.approveAudio(['mic'], []));
});

test('recording uses persisted defaults while retaining an explicit session destination and overrides', async () => {
	const commands = [];
	const defaults = { maxBytes: '2097152', maxDurationMs: '60000', maxSegments: 2, maxTotalBytes: '4194304' };
	const status = { id: '4', phase: 'Recording', failure: null, frames: '0', bytes: '0', durationMs: '0', segments: 1 };
	const service = new NativeRuntimeService(fakeEngine({
		engineCommand: command => {
			commands.push(command);
			return Promise.resolve({ type: 'recordingStatus', status });
		}
	}), unusedHost, unusedRecording, { snapshot: { values: { 'recording.defaults': defaults } } });
	await service.startRecording({ id: '1', generation: '7' }, { destination: '/chosen/session.mkv' });
	assert.deepEqual(commands[0], { type: 'startRecording', sessionId: '1', generation: '7', destination: '/chosen/session.mkv', maxBytes: defaults.maxBytes, maxDurationMs: defaults.maxDurationMs, retention: { maxSegments: defaults.maxSegments, maxTotalBytes: defaults.maxTotalBytes } });
	await service.startRecording({ id: '1', generation: '7' }, { destination: '/another.mkv', maxDurationMs: '120000' });
	assert.equal(commands[1].maxDurationMs, '120000');
	assert.equal(commands[1].maxBytes, defaults.maxBytes);
});

test('recording without a session destination resolves the persisted default folder through main-process integration', async () => {
	const calls = [];
	const service = new NativeRuntimeService(fakeEngine({
		engineCommand: async command => {
			calls.push(command);
			return { type: 'recordingStatus', status: {} };
		}
	}), unusedHost, {
		getDefaultDirectory: () => assert.fail('Unexpected folder read'),
		createDestination: async () => {
			calls.push('destination');
			return '/Pictures/VniLink/new.mkv';
		},
	}, configuration);
	await service.startRecording({ id: '1', generation: '2' }, {});
	assert.equal(calls[0], 'destination');
	assert.equal(calls[1].destination, '/Pictures/VniLink/new.mkv');
});

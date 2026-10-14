import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';
const result = await build({ stdin: { contents: `export { RemoteDesktopService } from './src/workbench/services/remoteDesktop/electron-browser/remoteDesktopService.js'; export { DesktopReadbackError, isValidDesktopQuality } from './src/workbench/services/remoteDesktop/common/remoteDesktopService.js'; export { isEngineCommand, isEngineReply } from './src/platform/engine/common/engine.js';`, loader: 'ts', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node', tsconfig: 'tsconfig.json' });
const { RemoteDesktopService, DesktopReadbackError, isValidDesktopQuality, isEngineCommand, isEngineReply } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const originalQuality = { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 };
function fixture() {
	const controller = { id: '1', generation: '7', role: 'Controller', phase: 'Active', peer_id: 'peer', quality: { ...originalQuality } };
	const host = { ...controller, role: 'Host', generation: '3', quality: { ...originalQuality } };
	const calls = []; let failReadback = false; let written = false;
	const api = {
		engineCommand: async command => {
			calls.push(command);
			if (written && failReadback) {
				throw new Error('Lost engine after commit');
			}
			if (command.type === 'controllerSessions') {
				return { type: 'sessions', sessions: [controller, { ...controller, id: '2', quality: null }] };
			}
			if (command.type === 'hostedSessions') {
				return { type: 'sessions', sessions: [host] };
			}
			if (command.type === 'snapshot') {
				return { type: 'snapshot', snapshot: { devices: [{ endpoint_id: 'peer', pet_name: 'Work', hostname: null }] } };
			}
			if (command.type === 'videoCodecs') {
				return { type: 'videoCodecs', codecs: { encode_baseline: false, encode_high: false, decode_baseline: true, decode_high: false, max_width: 3840, max_height: 2160, hardware: true, device: 'test', chroma444: false } };
			}
			if (command.type === 'sessionTopology' || command.type === 'topology') {
				return { type: 'displays', displays: [] };
			}
			if (command.type === 'updateSessionQuality') {
				(command.role === 'Controller' ? controller : host).quality = { ...command.quality }; written = true; return { type: 'done' };
			}
			throw new Error('Unexpected command');
		}
	};
	return {
		controller, host, calls, service: new RemoteDesktopService(api, true), failReadback: () => {
			failReadback = true;
		}
	};
}
test('desktop sessions preserve role and generation and omit non-desktop sessions', async () => {
	const f = fixture(); const settings = await f.service.load(); assert.equal(settings.sessions.length, 2); assert.equal(settings.sessions[0].name, 'Work');
	await f.service.displays(settings.sessions[0]); assert(f.calls.some(c => c.type === 'sessionTopology' && c.sessionId === '1' && c.generation === '7'));
	await f.service.displays(settings.sessions[1]); assert(f.calls.some(c => c.type === 'topology'));
	const quality = { ...originalQuality, max_frame_rate: 60 }; const state = await f.service.apply(settings.sessions[1], quality);
	assert.deepEqual(f.controller.quality, originalQuality); assert.deepEqual(state.sessions[1].quality, quality); assert(f.calls.some(c => c.type === 'updateSessionQuality' && c.role === 'Host' && c.generation === '3'));
});
test('reconnected sessions and invalid limits never receive a write', async () => {
	const f = fixture(); const target = (await f.service.load()).sessions[0]; f.controller.generation = '8';
	await assert.rejects(f.service.apply(target, originalQuality)); await assert.rejects(f.service.apply(target, { ...originalQuality, max_frame_rate: 61 })); assert(!f.calls.some(c => c.type === 'updateSessionQuality'));
	assert(!isValidDesktopQuality({ ...originalQuality, max_bitrate_kbps: 0 })); assert(!isValidDesktopQuality({ ...originalQuality, max_width: 7681 }));
});
test('quality drafts are captured and accepted writes with missing readback are explicit', async () => {
	const f = fixture(); const target = (await f.service.load()).sessions[0]; const draft = { ...originalQuality, max_frame_rate: 24 }; const saving = f.service.apply(target, draft); draft.max_frame_rate = 60; await saving; assert.equal(f.controller.quality.max_frame_rate, 24);
	const other = fixture(); const owner = (await other.service.load()).sessions[0]; other.failReadback(); await assert.rejects(other.service.apply(owner, originalQuality), DesktopReadbackError);
});
test('quality IPC requires a generation and rejects malformed negotiated quality', () => {
	const command = { type: 'updateSessionQuality', sessionId: '1', generation: '7', role: 'Controller', quality: originalQuality }; assert(isEngineCommand(command)); const { generation, ...missing } = command; assert.equal(generation, '7'); assert(!isEngineCommand(missing));
	assert(!isEngineReply({ type: 'sessions', sessions: [{ id: '1', generation: '7', role: 'Controller', peer_id: 'peer', phase: 'Active', quality: { bad: true } }] }));
});

test('codec preferences reject malformed or extra fields', () => {
	for (const codec of ['auto', 'h264', 'h265', 'vp8', 'vp9', 'av1']) {
		assert(isEngineCommand({ type: 'setDesktopCodecDefault', codec }));
	}
	for (const command of [{ type: 'setDesktopCodecDefault', codec: 'bogus' }, { type: 'setDesktopCodecDefault' }, { type: 'setDesktopCodecDefault', codec: 'vp9', sessionId: '1' }]) {
		assert(!isEngineCommand(command));
	}
});

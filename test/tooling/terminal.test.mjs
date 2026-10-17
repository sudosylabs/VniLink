import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeEngine } from './fixtures.mjs';
import { build } from 'esbuild';

const bundle = await build({ stdin: { contents: `export { TerminalSettingsService } from './src/workbench/services/terminal/electron-browser/terminalSettingsService.js'; export { validTerminalPolicy } from './src/workbench/services/terminal/common/terminalSettingsService.js';`, resolveDir: process.cwd(), loader: 'ts' }, bundle: true, write: false, format: 'esm', platform: 'node', tsconfig: 'tsconfig.json' });

const { TerminalSettingsService, validTerminalPolicy } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

const initial = { allow_persistence: true, max_sessions: 16, max_sessions_per_peer: 16, history_bytes: 1048576, replay_bytes: 8192, detached_timeout_secs: 300 };

test('terminal limits enforce engine bounds, capture drafts and reject failed readback', async () => {
	let policy = { ...initial };
	let written = false;
	let failRead = false;
	const calls = [];
	const service = new TerminalSettingsService(fakeEngine({
		engineCommand: async command => {
			calls.push(command);
			if (command.type === 'setTerminalPolicy') {
				policy = { ...command.policy };
				written = true;
				return { type: 'done' };
			}
			if (failRead && written) {
				throw new Error('Readback lost');
			}
			return { type: 'terminalPolicy', policy };
		}
	}, true));
	for (const invalid of [{ max_sessions: 17 }, { max_sessions_per_peer: 0 }, { history_bytes: 16383 }, { replay_bytes: 1048577 }, { detached_timeout_secs: 86401 }]) {
		assert(!validTerminalPolicy({ ...initial, ...invalid }));
		await assert.rejects(service.save({ ...initial, ...invalid }));
	}
	assert.equal(calls.length, 0);
	const draft = { ...initial, max_sessions_per_peer: 4 };
	const saving = service.save(draft);
	draft.max_sessions_per_peer = 8;
	assert.equal((await saving).max_sessions_per_peer, 4);
	failRead = true;
	await assert.rejects(service.save(initial));
	assert(!calls.some(command => command.type === 'enableDefaultNativeTerminal'));
});

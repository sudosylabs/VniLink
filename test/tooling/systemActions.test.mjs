import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const result = await build({
	stdin: {
		contents: `
			export { isEngineCommand, isEngineEvent, isEngineReply } from './src/platform/engine/common/engine.js';
			export { isMessageBoxOptions } from './src/platform/native/common/native.js';
			export { SystemActionNotices } from './src/platform/systemActions/electron-main/systemActionNotice.js';
			export { systemActionMessage, systemRequestMessage } from './src/workbench/services/systemActions/systemActionServiceImpl.js';
			export { Emitter } from './src/base/common/event.js';
		`,
		loader: 'ts',
		resolveDir: process.cwd(),
	},
	bundle: true,
	write: false,
	format: 'esm',
	platform: 'node',
	tsconfig: 'tsconfig.json',
	plugins: [{
		name: 'stubs',
		setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'stub' }));
			builder.onResolve({ filter: /(engineMainService|extensions)\.js$/ }, args => ({ path: args.path, namespace: 'stub' }));
			builder.onLoad({ filter: /.*/, namespace: 'stub' }, () => ({
				contents: 'export const app = { focus() {} }; export const dialog = {}; export const IEngineMainService = () => {}; export const registerSingleton = () => {}; export const InstantiationType = {};',
				loader: 'js',
			}));
		},
	}],
});

const { isEngineCommand, isEngineEvent, isEngineReply, isMessageBoxOptions, SystemActionNotices, systemActionMessage, systemRequestMessage, Emitter } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

const settle = () => new Promise(resolve => setTimeout(resolve, 10));

test('system commands, replies and events carry only what they name', () => {
	assert(isEngineCommand({ type: 'systemAction', sessionId: '1', generation: '2', action: 'restart' }));
	assert(!isEngineCommand({ type: 'systemAction', sessionId: '1', generation: '2', action: 'format' }));
	assert(!isEngineCommand({ type: 'systemAction', sessionId: '1', generation: '2', action: 'lock', force: true }));
	assert(isEngineCommand({ type: 'cancelSystemAction', sessionId: '1', generation: '2', id: '7' }));
	assert(isEngineCommand({ type: 'runHostedSystemActionNow', id: '7' }));
	assert(!isEngineCommand({ type: 'cancelHostedSystemAction', id: 'seven' }));
	const operation = { id: '7', action: 'restart', state: 'scheduled', detail: 'scheduled', noticeRemainingMs: 29000 };
	assert(isEngineReply({ type: 'systemOperation', operation }));
	assert(!isEngineReply({ type: 'systemOperation', operation: { ...operation, state: 'rebooting' } }));
	assert(isEngineReply({ type: 'scheduledSystemActions', actions: [{ id: '7', action: 'shutdown', endpoint_id: 'peer', remainingMs: 0 }] }));
	assert(!isEngineReply({ type: 'scheduledSystemActions', actions: [{ id: '7', action: 'lock', endpoint_id: 'peer', remainingMs: 0 }] }), 'a lock is never scheduled');
	assert(isEngineEvent({ type: 'systemActionScheduled', id: '7', action: 'restart', endpoint_id: 'peer', remainingMs: 30000 }));
	assert(isEngineEvent({ type: 'systemActionEnded', id: '7', state: 'cancelled' }));
	assert(!isEngineEvent({ type: 'systemActionEnded', id: '7', state: 'cancelled', by: 'peer' }));
});

test('a window shows only a short message box with a few plain buttons', () => {
	assert(isMessageBoxOptions({ type: 'warning', message: 'Restart Sudosy?', detail: 'Unsaved work may be lost.', buttons: ['Restart', 'Shut Down', 'Cancel'], defaultId: 2, cancelId: 2 }, 3));
	assert(!isMessageBoxOptions({ message: 'x', buttons: ['1', '2', '3', '4', '5'] }, 3), 'five buttons');
	assert(!isMessageBoxOptions({ message: 'x'.repeat(501) }, 3), 'a long message');
	assert(!isMessageBoxOptions({ message: 'x', buttons: ['OK'], cancelId: 1 }, 3), 'a cancel id past the buttons');
	assert(!isMessageBoxOptions({ message: 'x', checkboxLabel: 'Always' }, 3), 'an option the window may not set');
	assert(!isMessageBoxOptions({ message: 'x', targetWindowId: 4 }, 3), 'another window');
	assert(!isMessageBoxOptions({ message: 'x', type: 'error' }, 3));
});

/** An engine that records commands and lets the test send events. */
function fakeEngine(scheduled = []) {
	const events = new Emitter();
	const availability = new Emitter();
	const commands = [];
	return {
		commands,
		events,
		availability,
		engine: {
			available: true,
			onDidReceiveEvents: events.event,
			onDidChangeAvailability: availability.event,
			execute: async command => {
				commands.push(command);
				switch (command.type) {
					case 'snapshot': return { type: 'snapshot', snapshot: { devices: [{ endpoint_id: 'peer', pet_name: null, hostname: 'Studio Mac' }] } };
					case 'scheduledSystemActions': return { type: 'scheduledSystemActions', actions: scheduled };
					default: return { type: 'done' };
				}
			},
		},
	};
}

test('the person at the computer can cancel a restart, or start it now', async () => {
	for (const [answer, expected] of [[0, 'cancelHostedSystemAction'], [1, 'runHostedSystemActionNow']]) {
		const { engine, events, commands } = fakeEngine();
		const prompts = [];
		const notices = new SystemActionNotices(engine, async request => {
			prompts.push(request);
			return answer;
		}, error => assert.fail(String(error)), () => Date.UTC(2026, 9, 19, 12, 0, 0));
		events.fire({ events: [{ type: 'systemActionScheduled', id: '7', action: 'restart', endpoint_id: 'peer', remainingMs: 30000 }], resync: false });
		await settle();
		assert.equal(prompts.length, 1);
		assert.equal(prompts[0].message, 'Studio Mac wants to restart this computer');
		// The moment it runs, to the second, since the box cannot count down.
		const at = new Date(Date.UTC(2026, 9, 19, 12, 0, 30)).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
		assert.equal(prompts[0].detail, `It will restart at ${at} unless you cancel. Save your work first.`);
		assert.deepEqual(prompts[0].buttons, ['Cancel', 'Restart Now']);
		assert.deepEqual(commands.at(-1), { type: expected, id: '7' });
		notices.dispose();
	}
});

test('a notice closes itself when the action ends another way, and a missed one is shown', async () => {
	const { engine, events, availability, commands } = fakeEngine([{ id: '9', action: 'shutdown', endpoint_id: 'unknown', remainingMs: 12000 }]);
	const shown = [];
	const notices = new SystemActionNotices(engine, (request, signal) => new Promise(resolve => {
		shown.push(request);
		signal.addEventListener('abort', () => resolve(undefined));
	}), error => assert.fail(String(error)));
	await settle();
	// Started while events lapsed: the resync at start shows it, with a generic name.
	assert.equal(shown.length, 1);
	assert.equal(shown[0].message, 'A paired computer wants to shut down this computer');
	assert.deepEqual(shown[0].buttons, ['Cancel', 'Shut Down Now']);
	// The same countdown arriving again is not shown twice.
	events.fire({ events: [{ type: 'systemActionScheduled', id: '9', action: 'shutdown', endpoint_id: 'unknown', remainingMs: 11000 }], resync: false });
	await settle();
	assert.equal(shown.length, 1);
	// Cancelled from the computer that asked: the box closes and answers nothing.
	events.fire({ events: [{ type: 'systemActionEnded', id: '9', state: 'cancelled' }], resync: false });
	await settle();
	assert(!commands.some(command => command.type === 'cancelHostedSystemAction' || command.type === 'runHostedSystemActionNow'));
	availability.fire(true);
	await settle();
	assert.equal(commands.filter(command => command.type === 'scheduledSystemActions').length, 2, 'an engine that comes back is asked again');
	notices.dispose();
});

test('the computer that asked reads every outcome in words', () => {
	const at = (action, state, detail = '', noticeRemainingMs = 0) => systemActionMessage({ action, state, detail, noticeRemainingMs });
	assert.equal(at('lock', 'executed'), 'Locked');
	assert.equal(at('restart', 'scheduled', 'scheduled', 23400), 'Restarting in 24 s');
	assert.equal(at('shutdown', 'scheduled', 'scheduled', 400), 'Shutting down in 1 s');
	assert.equal(at('restart', 'executed'), 'Restart started');
	assert.equal(at('restart', 'cancelled', 'cancelled at that computer'), 'Cancelled there');
	assert.equal(at('restart', 'cancelled', 'the computer that asked disconnected'), 'Cancelled: disconnected');
	assert.equal(at('shutdown', 'cancelled', 'cancelled by the computer that asked'), 'Cancelled');
	assert.equal(at('restart', 'refused', 'disabled by host policy'), 'Turned off there');
	assert.equal(at('lock', 'refused', 'system permission revoked'), 'Not allowed there');
	assert.equal(at('shutdown', 'failed', 'osascript failed'), 'Couldn\'t shut down');
	assert.equal(systemRequestMessage(new Error('Unauthorized: denied')), 'Not allowed there');
	assert.equal(systemRequestMessage(new Error('NoSession: gone')), 'Not connected');
	assert.equal(systemRequestMessage(new Error('boom')), 'Couldn\'t reach it');
});

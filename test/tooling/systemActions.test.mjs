import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const result = await build({
	stdin: {
		contents: `
			export { isEngineCommand, isEngineEvent, isEngineReply } from './src/platform/engine/common/engine.js';
			export { isMessageBoxOptions } from './src/platform/native/common/native.js';
			export { SystemActionNotices } from './src/platform/systemActions/electron-main/systemActionNotice.js';
			export { noticeCountdown, noticeTitle } from './src/platform/systemActions/common/noticeText.js';
			export { isNoticeAnswer, isNoticeContent } from './src/platform/systemActions/common/notice.js';
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
				contents: 'export const app = { focus() {} }; export const BrowserWindow = class {}; export const nativeTheme = {}; export const screen = {}; export const IEngineMainService = () => {}; export const registerSingleton = () => {}; export const InstantiationType = {};',
				loader: 'js',
			}));
		},
	}],
});

const { isEngineCommand, isEngineEvent, isEngineReply, isMessageBoxOptions, isNoticeAnswer, isNoticeContent, noticeCountdown, noticeTitle, SystemActionNotices, systemActionMessage, systemRequestMessage, Emitter } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

const settle = () => new Promise(resolve => setTimeout(resolve, 10));

test('system commands, replies and events carry only what they name', () => {
	assert(isEngineCommand({ type: 'systemAction', sessionId: '1', generation: '2', action: 'restart' }));
	assert(!isEngineCommand({ type: 'systemAction', sessionId: '1', generation: '2', action: 'format' }));
	assert(!isEngineCommand({ type: 'systemAction', sessionId: '1', generation: '2', action: 'lock', force: true }));
	assert(isEngineCommand({ type: 'cancelSystemAction', sessionId: '1', generation: '2', id: '7' }));
	assert(isEngineCommand({ type: 'setSystemPrivacy', sessionId: '1', generation: '2', enabled: true }));
	assert(!isEngineCommand({ type: 'setSystemPrivacy', sessionId: '1', generation: '2', enabled: 'on' }));
	assert(!isEngineCommand({ type: 'setSystemPrivacy', sessionId: '1', generation: '2', enabled: true, inputBlocked: false }), 'privacy mode is one switch');
	assert(isEngineCommand({ type: 'systemPrivacy', sessionId: '1', generation: '2' }));
	assert(isEngineReply({ type: 'systemPrivacy', privacy: { displayPrivate: true, inputBlocked: true } }));
	assert(!isEngineReply({ type: 'systemPrivacy', privacy: { displayPrivate: true, inputBlocked: true, holder: '3' } }), 'the holder stays on the other computer');
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
	assert(isMessageBoxOptions({ type: 'warning', message: 'Restart Studio PC?', detail: 'Unsaved work may be lost.', buttons: ['Restart', 'Shut Down', 'Cancel'], defaultId: 2, cancelId: 2 }, 3));
	assert(!isMessageBoxOptions({ message: 'x', buttons: ['1', '2', '3', '4', '5'] }, 3), 'five buttons');
	assert(!isMessageBoxOptions({ message: 'x'.repeat(501) }, 3), 'a long message');
	assert(!isMessageBoxOptions({ message: 'x', buttons: ['OK'], cancelId: 1 }, 3), 'a cancel id past the buttons');
	assert(isMessageBoxOptions({ message: 'Replace notes.txt?', buttons: ['Replace', 'Skip'], checkboxLabel: 'Do the same for the other conflicts', checkboxChecked: true }, 3), 'a checkbox');
	assert(!isMessageBoxOptions({ message: 'x', checkboxLabel: '' }, 3), 'an empty checkbox label');
	assert(!isMessageBoxOptions({ message: 'x', checkboxLabel: 'x'.repeat(101) }, 3), 'a long checkbox label');
	assert(!isMessageBoxOptions({ message: 'x', checkboxLabel: 'Always', checkboxChecked: 'yes' }, 3), 'a checkbox state that is not a boolean');
	assert(!isMessageBoxOptions({ message: 'x', checkboxChecked: true }, 3), 'a checkbox state without a checkbox');
	assert(!isMessageBoxOptions({ message: 'x', textWidth: 300 }, 3), 'an option the window may not set');
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
		const notices = new SystemActionNotices(engine, async content => {
			prompts.push(content);
			return answer;
		}, error => assert.fail(String(error)), () => 1_000_000);
		events.fire({ events: [{ type: 'systemActionScheduled', id: '7', action: 'restart', endpoint_id: 'peer', remainingMs: 30000 }], resync: false });
		await settle();
		// The window counts down to the deadline itself.
		assert.deepEqual(prompts, [{ action: 'restart', name: 'Studio Mac', deadline: 1_030_000 }]);
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
	assert.equal(shown[0].action, 'shutdown');
	assert.equal(shown[0].name, 'A paired computer');
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

test('the notice names who asked and counts down in whole seconds', () => {
	const restart = { action: 'restart', name: 'Studio Mac', deadline: 30_000 };
	assert.equal(noticeTitle(restart), 'Studio Mac wants to restart this computer');
	assert.equal(noticeTitle({ ...restart, action: 'shutdown' }), 'Studio Mac wants to shut down this computer');
	assert.equal(noticeCountdown(restart, 0), 'Restarting in 30 s. Save your work, or cancel.');
	assert.equal(noticeCountdown(restart, 5_100), 'Restarting in 25 s. Save your work, or cancel.');
	assert.equal(noticeCountdown({ ...restart, action: 'shutdown' }, 40_000), 'Shutting down in 0 s. Save your work, or cancel.');
	assert(isNoticeContent(restart));
	assert(!isNoticeContent({ ...restart, action: 'lock' }), 'a lock never counts down');
	assert(!isNoticeContent({ ...restart, extra: true }));
	assert(isNoticeAnswer('cancel') && isNoticeAnswer('now') && !isNoticeAnswer('later'));
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
	assert.equal(systemRequestMessage(new Error('Busy: one at a time')), 'Busy, try again');
	assert.equal(systemRequestMessage(new Error('Timeout: no reply')), 'No answer in time');
	assert.equal(systemRequestMessage(new Error('Unsupported: input blocking')), 'Not possible there', 'a computer that refuses was reached');
	assert.equal(systemRequestMessage(new Error('boom')), 'Couldn\'t reach it');
});

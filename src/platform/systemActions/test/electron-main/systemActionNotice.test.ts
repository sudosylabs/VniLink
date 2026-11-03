// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../base/common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import type { EngineCommand, EngineEvent, EngineReply } from '../../../engine/common/engine.js';
import { engineDevice, snapshotReply, TestEngineService } from '../../../engine/test/common/testEngineService.js';
import type { NoticeContent } from '../../common/notice.js';
import { SystemActionNotices, type NoticePrompt } from '../../electron-main/systemActionNotice.js';

type ScheduledAction = Extract<EngineReply, { type: 'scheduledSystemActions' }>['actions'][number];

/** The engine's replies: one paired computer named Studio Mac, and `scheduled` running countdowns. */
function engineReplies(scheduled: readonly ScheduledAction[] = []): (command: EngineCommand) => EngineReply {
	return command => {
		if (command.type === 'snapshot') {
			return snapshotReply([engineDevice('peer', { hostname: 'Studio Mac' })]);
		}
		if (command.type === 'scheduledSystemActions') {
			return { type: 'scheduledSystemActions', actions: [...scheduled] };
		}
		return { type: 'done' };
	};
}

function scheduled(id: string, action: 'restart' | 'shutdown', endpointId = 'peer'): EngineEvent {
	return { type: 'systemActionScheduled', id, action, endpoint_id: endpointId, remainingMs: 30000 };
}

/** Whether the notice answered the engine for the person at this computer. */
function answered(engine: TestEngineService): boolean {
	return engine.commands.some(command => command.type === 'cancelHostedSystemAction' || command.type === 'runHostedSystemActionNow');
}

suite('SystemActionNotices', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let engine: TestEngineService;
	let errors: unknown[];
	/** The notices shown and not yet closed, in the order they opened. */
	let open: NoticeContent[];
	/** Shows a notice until it is closed, as the notice window does when nobody answers. */
	const unanswered: NoticePrompt = (content, signal) => new Promise(resolve => {
		open.push(content);
		signal.addEventListener('abort', () => {
			open.splice(open.indexOf(content), 1);
			resolve(undefined);
		});
	});
	function createNotices(prompt: NoticePrompt, now?: () => number): SystemActionNotices {
		return disposables.add(new SystemActionNotices(engine, prompt, error => errors.push(error), now));
	}
	setup(() => {
		engine = disposables.add(new TestEngineService(engineReplies()));
		errors = [];
		open = [];
	});
	teardown(() => {
		assert.deepStrictEqual(errors, []);
	});
	for (const [answer, choice, expected] of [[0, 'cancel a restart', 'cancelHostedSystemAction'], [1, 'start a restart now', 'runHostedSystemActionNow']] as const) {
		test(`the person at the computer can ${choice}`, async () => {
			createNotices(() => Promise.resolve(answer));
			engine.fireEvents({ events: [scheduled('7', 'restart')], resync: false });
			await timeout(0);
			assert.deepStrictEqual(engine.commands.at(-1), { type: expected, id: '7' });
		});
	}
	test('the notice names who asked and the deadline the window counts down to', async () => {
		const prompts: NoticeContent[] = [];
		createNotices(content => {
			prompts.push(content);
			return Promise.resolve(undefined);
		}, () => 1_000_000);
		engine.fireEvents({ events: [scheduled('7', 'restart')], resync: false });
		await timeout(0);
		assert.deepStrictEqual(prompts, [{ action: 'restart', name: 'Studio Mac', deadline: 1_030_000 }]);
	});
	test('a countdown that began while events lapsed is shown when the notices start, with a generic name', async () => {
		engine.handler = engineReplies([{ id: '9', action: 'shutdown', endpoint_id: 'unknown', remainingMs: 12000 }]);
		createNotices(unanswered);
		await timeout(0);
		assert.deepStrictEqual(open.map(({ action, name }) => ({ action, name })), [{ action: 'shutdown', name: 'A paired computer' }]);
	});
	test('the same countdown arriving again is not shown twice', async () => {
		engine.handler = engineReplies([{ id: '9', action: 'shutdown', endpoint_id: 'unknown', remainingMs: 12000 }]);
		createNotices(unanswered);
		await timeout(0);
		engine.fireEvents({ events: [{ type: 'systemActionScheduled', id: '9', action: 'shutdown', endpoint_id: 'unknown', remainingMs: 11000 }], resync: false });
		await timeout(0);
		assert.strictEqual(open.length, 1);
	});
	test('an action cancelled from the computer that asked closes its notice and answers nothing', async () => {
		createNotices(unanswered);
		engine.fireEvents({ events: [scheduled('9', 'shutdown')], resync: false });
		await timeout(0);
		engine.fireEvents({ events: [{ type: 'systemActionEnded', id: '9', state: 'cancelled' }], resync: false });
		await timeout(0);
		assert.deepStrictEqual(open, []);
		assert.strictEqual(answered(engine), false);
	});
	test('an engine that comes back is asked again for running countdowns', async () => {
		createNotices(unanswered);
		await timeout(0);
		engine.setAvailable(true);
		await timeout(0);
		assert.strictEqual(engine.commands.filter(command => command.type === 'scheduledSystemActions').length, 2);
	});
	test('quitting closes an open notice without answering it', async () => {
		const notices = createNotices(unanswered);
		engine.fireEvents({ events: [scheduled('3', 'restart')], resync: false });
		await timeout(0);
		assert.strictEqual(open.length, 1);
		notices.close();
		await timeout(0);
		// The notice window cannot be closed by the system, so one left open would cancel the quit.
		assert.deepStrictEqual(open, []);
		assert.strictEqual(answered(engine), false);
	});
	test('no notice opens after quitting', async () => {
		const notices = createNotices(unanswered);
		notices.close();
		engine.fireEvents({ events: [scheduled('4', 'shutdown')], resync: false });
		await timeout(0);
		assert.deepStrictEqual(open, []);
	});
});

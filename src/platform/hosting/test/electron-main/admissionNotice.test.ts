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
import type { NoticeContent } from '../../../systemActions/common/notice.js';
import type { NoticePrompt } from '../../../systemActions/electron-main/systemActionNotice.js';
import { AdmissionNotices } from '../../electron-main/admissionNotice.js';

type WaitingRequest = Extract<EngineReply, { type: 'admissionRequests' }>['requests'][number];

/** The engine's replies: one paired computer named Studio Mac, and `waiting` requests. */
function engineReplies(waiting: readonly WaitingRequest[] = []): (command: EngineCommand) => EngineReply {
	return command => {
		if (command.type === 'snapshot') {
			return snapshotReply([engineDevice('peer', { hostname: 'Studio Mac' })]);
		}
		if (command.type === 'admissionRequests') {
			return { type: 'admissionRequests', requests: [...waiting] };
		}
		return { type: 'done' };
	};
}

function requested(id: string, endpointId = 'peer'): EngineEvent {
	return { type: 'admissionRequested', id, endpoint_id: endpointId, remainingMs: 30000 };
}

/** The answers the notices gave the engine. */
function answers(engine: TestEngineService): EngineCommand[] {
	return engine.commands.filter(command => command.type === 'answerAdmissionRequest');
}

suite('AdmissionNotices', () => {
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
	function createNotices(prompt: NoticePrompt, now?: () => number): AdmissionNotices {
		return disposables.add(new AdmissionNotices(engine, prompt, error => errors.push(error), now));
	}
	setup(() => {
		engine = disposables.add(new TestEngineService(engineReplies()));
		errors = [];
		open = [];
	});
	teardown(() => {
		assert.deepStrictEqual(errors, []);
	});
	for (const [answer, choice, accept] of [[0, 'decline', false], [1, 'allow', true]] as const) {
		test(`the person at the computer can ${choice} a computer asking to use it`, async () => {
			createNotices(() => Promise.resolve(answer));
			engine.fireEvents({ events: [requested('7')], resync: false });
			await timeout(0);
			assert.deepStrictEqual(answers(engine), [{ type: 'answerAdmissionRequest', id: '7', accept }]);
		});
	}
	test('the notice names who asks and when it is declined unanswered', async () => {
		const prompts: NoticeContent[] = [];
		createNotices(content => {
			prompts.push(content);
			return Promise.resolve(undefined);
		}, () => 1_000_000);
		engine.fireEvents({ events: [requested('7')], resync: false });
		await timeout(0);
		assert.deepStrictEqual(prompts, [{ action: 'admit', name: 'Studio Mac', deadline: 1_030_000 }]);
		assert.deepStrictEqual(answers(engine), [], 'a dismissed notice answers nothing; the engine declines at the deadline');
	});
	test('a request that began while events lapsed is shown when the notices start, once', async () => {
		engine.handler = engineReplies([{ id: '9', endpoint_id: 'peer', remainingMs: 12000 }]);
		createNotices(unanswered);
		await timeout(0);
		engine.fireEvents({ events: [requested('9')], resync: false });
		await timeout(0);
		assert.deepStrictEqual(open.map(({ action, name }) => ({ action, name })), [{ action: 'admit', name: 'Studio Mac' }]);
	});
	test('a request that ends another way closes its notice and answers nothing', async () => {
		createNotices(unanswered);
		engine.fireEvents({ events: [requested('9')], resync: false });
		await timeout(0);
		engine.fireEvents({ events: [{ type: 'admissionRequestEnded', id: '9' }], resync: false });
		await timeout(0);
		assert.deepStrictEqual(open, []);
		assert.deepStrictEqual(answers(engine), []);
	});
	test('quitting closes an open notice without answering it, and none opens after', async () => {
		const notices = createNotices(unanswered);
		engine.fireEvents({ events: [requested('3')], resync: false });
		await timeout(0);
		notices.close();
		engine.fireEvents({ events: [requested('4')], resync: false });
		await timeout(0);
		assert.deepStrictEqual(open, []);
		assert.deepStrictEqual(answers(engine), []);
	});
});

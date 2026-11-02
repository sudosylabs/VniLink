// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isNoticeAnswer, isNoticeContent, type NoticeContent } from '../../common/notice.js';
import { noticeCountdown, noticeTitle } from '../../common/noticeText.js';

const restart: NoticeContent = { action: 'restart', name: 'Studio Mac', deadline: 30_000 };

suite('System action notice', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('the title names who asks to restart', () => {
		assert.strictEqual(noticeTitle(restart), 'Studio Mac wants to restart this computer');
	});
	test('the title names who asks to shut down', () => {
		assert.strictEqual(noticeTitle({ ...restart, action: 'shutdown' }), 'Studio Mac wants to shut down this computer');
	});
	test('the countdown starts at the whole time left', () => {
		assert.strictEqual(noticeCountdown(restart, 0), 'Restarting in 30 s. Save your work, or cancel.');
	});
	test('the countdown counts in whole seconds', () => {
		assert.strictEqual(noticeCountdown(restart, 5_100), 'Restarting in 25 s. Save your work, or cancel.');
	});
	test('the countdown past its deadline reads zero', () => {
		assert.strictEqual(noticeCountdown({ ...restart, action: 'shutdown' }, 40_000), 'Shutting down in 0 s. Save your work, or cancel.');
	});
	test('a restart notice is valid content', () => {
		assert.strictEqual(isNoticeContent(restart), true);
	});
	test('a lock never counts down', () => {
		assert.strictEqual(isNoticeContent({ ...restart, action: 'lock' }), false);
	});
	test('content with an extra field is refused', () => {
		assert.strictEqual(isNoticeContent({ ...restart, extra: true }), false);
	});
	for (const [answer, valid] of [['cancel', true], ['now', true], ['later', false]] as const) {
		test(`the answer '${answer}' is ${valid ? 'accepted' : 'refused'}`, () => {
			assert.strictEqual(isNoticeAnswer(answer), valid);
		});
	}
});

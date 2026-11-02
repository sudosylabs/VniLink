// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import type { SystemOperation } from '../../../../../platform/engine/common/engine.js';
import { systemActionMessage, systemRequestMessage } from '../../systemActionServiceImpl.js';

type Outcome = Pick<SystemOperation, 'action' | 'state' | 'detail' | 'noticeRemainingMs'>;

/** Outcomes the engine reports to the computer that asked, and the words it shows for each. */
const outcomes: readonly (readonly [Outcome, string])[] = [
	[{ action: 'lock', state: 'executed', detail: '', noticeRemainingMs: 0 }, 'Locked'],
	[{ action: 'restart', state: 'scheduled', detail: 'scheduled', noticeRemainingMs: 23400 }, 'Restarting in 24 s'],
	[{ action: 'shutdown', state: 'scheduled', detail: 'scheduled', noticeRemainingMs: 400 }, 'Shutting down in 1 s'],
	[{ action: 'restart', state: 'executed', detail: '', noticeRemainingMs: 0 }, 'Restart started'],
	[{ action: 'restart', state: 'cancelled', detail: 'cancelled at that computer', noticeRemainingMs: 0 }, 'Cancelled there'],
	[{ action: 'restart', state: 'cancelled', detail: 'the computer that asked disconnected', noticeRemainingMs: 0 }, 'Cancelled: disconnected'],
	[{ action: 'shutdown', state: 'cancelled', detail: 'cancelled by the computer that asked', noticeRemainingMs: 0 }, 'Cancelled'],
	[{ action: 'restart', state: 'refused', detail: 'disabled by host policy', noticeRemainingMs: 0 }, 'Turned off there'],
	[{ action: 'lock', state: 'refused', detail: 'system permission revoked', noticeRemainingMs: 0 }, 'Not allowed there'],
	[{ action: 'shutdown', state: 'failed', detail: 'osascript failed', noticeRemainingMs: 0 }, 'Couldn\'t shut down'],
];

/** Requests that failed before any outcome, and the words they show. */
const failures: readonly (readonly [string, string])[] = [
	['Unauthorized: denied', 'Not allowed there'],
	['NoSession: gone', 'Not connected'],
	['Busy: one at a time', 'Busy, try again'],
	['Timeout: no reply', 'No answer in time'],
	// A computer that refuses was reached.
	['Unsupported: input blocking', 'Not possible there'],
	['boom', 'Couldn\'t reach it'],
];

suite('System action messages', () => {
	for (const [operation, message] of outcomes) {
		test(`a ${operation.action} ${operation.state}${operation.detail ? ` (${operation.detail})` : ''} reads '${message}'`, () => {
			assert.strictEqual(systemActionMessage(operation), message);
		});
	}
	for (const [error, message] of failures) {
		test(`a request that failed with '${error}' reads '${message}'`, () => {
			assert.strictEqual(systemRequestMessage(new Error(error)), message);
		});
	}
	ensureNoDisposablesAreLeakedInTestSuite();
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isEngineCommand, isEngineEvent, isEngineReply } from '../../common/engine.js';

const operation = { id: '7', action: 'restart', state: 'scheduled', detail: 'scheduled', noticeRemainingMs: 29000 };

/** Each case names one message and whether the guard accepts it. */
const commands: readonly (readonly [string, unknown, boolean])[] = [
	['a restart asked of a session', { type: 'systemAction', sessionId: '1', generation: '2', action: 'restart' }, true],
	['an action the engine does not know', { type: 'systemAction', sessionId: '1', generation: '2', action: 'format' }, false],
	['an action with an option it does not take', { type: 'systemAction', sessionId: '1', generation: '2', action: 'lock', force: true }, false],
	['the cancellation of an action', { type: 'cancelSystemAction', sessionId: '1', generation: '2', id: '7' }, true],
	['privacy mode switched on', { type: 'setSystemPrivacy', sessionId: '1', generation: '2', enabled: true }, true],
	['privacy mode set to a word', { type: 'setSystemPrivacy', sessionId: '1', generation: '2', enabled: 'on' }, false],
	['privacy mode with a second switch, though privacy mode is one switch', { type: 'setSystemPrivacy', sessionId: '1', generation: '2', enabled: true, inputBlocked: false }, false],
	['a privacy mode read', { type: 'systemPrivacy', sessionId: '1', generation: '2' }, true],
	['a hosted action started now', { type: 'runHostedSystemActionNow', id: '7' }, true],
	['a hosted action cancelled by a word id', { type: 'cancelHostedSystemAction', id: 'seven' }, false],
];

const replies: readonly (readonly [string, unknown, boolean])[] = [
	['the privacy mode of the other computer', { type: 'systemPrivacy', privacy: { displayPrivate: true, inputBlocked: true } }, true],
	['the privacy mode with its holder, who stays on the other computer', { type: 'systemPrivacy', privacy: { displayPrivate: true, inputBlocked: true, holder: '3' } }, false],
	['a scheduled operation', { type: 'systemOperation', operation }, true],
	['an operation in a state the engine does not report', { type: 'systemOperation', operation: { ...operation, state: 'rebooting' } }, false],
	['a scheduled shutdown', { type: 'scheduledSystemActions', actions: [{ id: '7', action: 'shutdown', endpoint_id: 'peer', remainingMs: 0 }] }, true],
	['a scheduled lock, though a lock is never scheduled', { type: 'scheduledSystemActions', actions: [{ id: '7', action: 'lock', endpoint_id: 'peer', remainingMs: 0 }] }, false],
];

const events: readonly (readonly [string, unknown, boolean])[] = [
	['a restart scheduled by a paired computer', { type: 'systemActionScheduled', id: '7', action: 'restart', endpoint_id: 'peer', remainingMs: 30000 }, true],
	['an action that ended', { type: 'systemActionEnded', id: '7', state: 'cancelled' }, true],
	['an action that ended, naming who ended it', { type: 'systemActionEnded', id: '7', state: 'cancelled', by: 'peer' }, false],
];

suite('Engine system action contract', () => {
	for (const [name, value, accepted] of commands) {
		test(`a command is ${accepted ? 'accepted' : 'refused'}: ${name}`, () => {
			assert.strictEqual(isEngineCommand(value), accepted);
		});
	}
	for (const [name, value, accepted] of replies) {
		test(`a reply is ${accepted ? 'accepted' : 'refused'}: ${name}`, () => {
			assert.strictEqual(isEngineReply(value), accepted);
		});
	}
	for (const [name, value, accepted] of events) {
		test(`an event is ${accepted ? 'accepted' : 'refused'}: ${name}`, () => {
			assert.strictEqual(isEngineEvent(value), accepted);
		});
	}
	ensureNoDisposablesAreLeakedInTestSuite();
});

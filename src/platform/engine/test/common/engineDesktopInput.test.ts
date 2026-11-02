// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isDesktopInputCommand, isEngineCommand } from '../../common/engine.js';

suite('Desktop input commands', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const target = { sessionId: '4', generation: '2' };
	const accepted: [string, unknown][] = [
		['a key with its composing state', { type: 'submitLocalKey', ...target, usage: 0x2f, down: true, text: '', composing: true }],
		['the displays the pointer lands on', { type: 'selectDisplays', ...target, ids: ['7'] }],
		['the Control and Command exchange, in order with the keys', { type: 'swapSessionModifiers', ...target, enabled: true }],
		['this computer\'s shortcuts going to the desktop while it has the keyboard', { type: 'captureSystemKeys', ...target, enabled: false }],
	];
	for (const [name, command] of accepted) {
		test(`the input channel takes ${name}`, () => {
			assert.strictEqual(isDesktopInputCommand(command), true);
		});
	}
	const refused: [string, unknown][] = [
		['a key that does not say whether it composes', { type: 'submitLocalKey', ...target, usage: 4, down: true, text: 'a' }],
		['a modifier exchange that is not on or off', { type: 'swapSessionModifiers', ...target, enabled: 'yes' }],
		['a quality change, being neither input nor display selection', { type: 'updateSessionQuality', ...target, role: 'Controller', quality: { max_bitrate_kbps: 1, max_frame_rate: 1, max_width: 1, max_height: 1 } }],
		['an input with an extra field', { type: 'submitKey', ...target, usage: 4, down: true, extra: 1 }],
	];
	for (const [name, command] of refused) {
		test(`the input channel refuses ${name}`, () => {
			assert.strictEqual(isDesktopInputCommand(command), false);
		});
	}
	const channelOnly = [
		{ type: 'submitKey', ...target, usage: 4, down: true },
		{ type: 'submitPointer', ...target, x: 1, y: 2, display: '7', revision: '1' },
		{ type: 'submitButton', ...target, button: 'Left', down: true },
		{ type: 'submitWheel', ...target, dx: 0, dy: -1 },
		{ type: 'submitMotion', ...target, dx: 1, dy: 0 },
		{ type: 'submitPreciseWheel', ...target, dx: 15, dy: -30 },
		{ type: 'submitText', ...target, text: 'a' },
		{ type: 'submitLocalKey', ...target, usage: 4, down: true, text: 'a', composing: false },
		{ type: 'releaseSessionInput', ...target },
		{ type: 'selectDisplays', ...target, ids: ['7'] },
	];
	for (const command of channelOnly) {
		test(`${command.type} is desktop input, not an engine command`, () => {
			assert.strictEqual(isDesktopInputCommand(command), true);
			assert.strictEqual(isEngineCommand(command), false);
		});
	}
	test('no engine command pastes: text crosses on a desktop\'s own clipboard pipe', () => {
		assert.strictEqual(isEngineCommand({ type: 'submitClipboard', ...target, text: 'a' }), false);
	});
});

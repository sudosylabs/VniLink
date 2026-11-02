// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { recordingStartFailure, type RecordingStartFailure } from '../../sessionRecording.js';

suite('recordingStartFailure', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const cases: readonly (readonly [string, RecordingStartFailure])[] = [
		['Unauthorized: recording is not allowed', 'notAllowed'],
		['NoSession: no camera is open on this connection', 'disconnected'],
		['Unsupported: recording storage is unavailable', 'failed'],
	];
	for (const [message, failure] of cases) {
		test(`a recording refused with "${message}" reports ${failure}`, () => {
			assert.strictEqual(recordingStartFailure(new Error(message)), failure);
		});
	}
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isPermissionCommand, isPermissionSnapshot, permissionKinds, type PermissionSnapshot } from '../../common/permissions.js';

/** A well-formed snapshot of a computer that has granted nothing. */
const snapshot: PermissionSnapshot = {
	development: false,
	permissions: permissionKinds.map(kind => ({ kind, status: 'denied', canRequest: false, canOpenSettings: true })),
};

suite('isPermissionCommand', () => {
	test('a status query is accepted', () => {
		assert.strictEqual(isPermissionCommand({ type: 'status' }), true);
	});
	test('a request for a known permission is accepted', () => {
		assert.strictEqual(isPermissionCommand({ type: 'request', kind: 'camera' }), true);
	});
	test('a settings command that names its own destination is refused', () => {
		assert.strictEqual(isPermissionCommand({ type: 'openSettings', kind: 'camera', url: 'https://example.com' }), false);
	});
	test('a request for a permission the computer does not have is refused', () => {
		assert.strictEqual(isPermissionCommand({ type: 'request', kind: 'files' }), false);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

suite('isPermissionSnapshot', () => {
	test('a snapshot listing every permission is accepted', () => {
		assert.strictEqual(isPermissionSnapshot(snapshot), true);
	});
	test('a snapshot with an extra field is refused', () => {
		assert.strictEqual(isPermissionSnapshot({ ...snapshot, extra: true }), false);
	});
	test('a snapshot without its permissions is refused', () => {
		assert.strictEqual(isPermissionSnapshot({ development: false, permissions: [] }), false);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

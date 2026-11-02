// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { permissionKinds, type PermissionKind, type PermissionSnapshot } from '../../../permissions/common/permissions.js';
import { synchronizeNativePermissions } from '../../electron-main/nativeRuntime.js';
import { TestEngineService } from '../common/testEngineService.js';

/** A snapshot in which only `granted` permissions are granted and the rest are denied. */
function permissions(...granted: PermissionKind[]): PermissionSnapshot {
	return {
		development: false,
		permissions: permissionKinds.map(kind => ({ kind, status: granted.includes(kind) ? 'granted' : 'denied', canRequest: false, canOpenSettings: true })),
	};
}

suite('synchronizeNativePermissions', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let engine: TestEngineService;
	setup(() => {
		engine = disposables.add(new TestEngineService(() => ({ type: 'done' })));
	});
	test('on a Mac without Accessibility consent, permissions are reported and input is not installed', async () => {
		await synchronizeNativePermissions(engine, 'darwin', permissions());
		assert.deepStrictEqual(engine.commands, [{ type: 'reportNativePermissions', screen: false, input: false, camera: false, microphone: false }]);
	});
	test('on a Mac with Accessibility consent, input is installed after the report', async () => {
		await synchronizeNativePermissions(engine, 'darwin', permissions('accessibility'));
		assert.deepStrictEqual(engine.commands.map(command => command.type), ['reportNativePermissions', 'enableNativeInput']);
	});
	test('each granted permission is reported as allowed', async () => {
		await synchronizeNativePermissions(engine, 'darwin', permissions('screen', 'camera', 'microphone'));
		assert.deepStrictEqual(engine.commands[0], { type: 'reportNativePermissions', screen: true, input: false, camera: true, microphone: true });
	});
	test('synchronizing never starts hosting or chooses camera sources', async () => {
		await synchronizeNativePermissions(engine, 'darwin', permissions(...permissionKinds));
		assert.deepStrictEqual(engine.commands.filter(command => command.type === 'startHosting' || command.type === 'setNativeCameraSources'), []);
	});
	for (const platform of ['win32', 'linux']) {
		test(`on ${platform}, where input needs no consent, input is installed`, async () => {
			await synchronizeNativePermissions(engine, platform, permissions());
			assert.deepStrictEqual(engine.commands, [{ type: 'enableNativeInput' }]);
		});
	}
	test('an unavailable engine is sent nothing', async () => {
		engine.setAvailable(false);
		await synchronizeNativePermissions(engine, 'darwin', permissions('accessibility'));
		assert.deepStrictEqual(engine.commands, []);
	});
});

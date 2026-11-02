// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isPermissionSnapshot, type PermissionKind } from '../../common/permissions.js';
import { MachinePermissions } from '../../electron-main/machinePermissions.js';

type MediaKind = 'screen' | 'camera' | 'microphone';

type MediaStatus = ReturnType<Electron.SystemPreferences['getMediaAccessStatus']>;

/**
 * The operating system's consent, as `systemPreferences` reports it. Every
 * prompt it is asked to show is kept in `prompts`; granting a media prompt
 * changes the status the next read sees.
 */
class TestPreferences implements Pick<Electron.SystemPreferences, 'getMediaAccessStatus' | 'askForMediaAccess' | 'isTrustedAccessibilityClient'> {
	public readonly states: Record<MediaKind, MediaStatus> = { screen: 'denied', camera: 'not-determined', microphone: 'granted' };
	public readonly prompts: string[] = [];
	public trusted = false;

	public getMediaAccessStatus(kind: MediaKind): MediaStatus {
		return this.states[kind];
	}

	public isTrustedAccessibilityClient(prompt: boolean): boolean {
		if (prompt) {
			this.prompts.push('accessibility');
		}
		return this.trusted;
	}

	public askForMediaAccess(kind: 'camera' | 'microphone'): Promise<boolean> {
		this.prompts.push(kind);
		this.states[kind] = 'granted';
		return Promise.resolve(true);
	}
}

suite('MachinePermissions', () => {
	let preferences: TestPreferences;
	/** The settings pages opened, in order. */
	let urls: string[];
	function createPermissions(platform: string): MachinePermissions {
		return new MachinePermissions(platform, true, preferences, url => {
			urls.push(url);
			return Promise.resolve();
		}, () => {
			preferences.prompts.push('screen');
			return Promise.resolve();
		});
	}
	function permission(permissions: MachinePermissions, kind: PermissionKind) {
		const found = permissions.snapshot().permissions.find(entry => entry.kind === kind);
		assert.ok(found);
		return found;
	}
	setup(() => {
		preferences = new TestPreferences();
		urls = [];
	});
	suite('macOS', () => {
		test('a status query reports what the operating system says, in a well-formed snapshot', async () => {
			const snapshot = await createPermissions('darwin').execute({ type: 'status' });
			assert.strictEqual(isPermissionSnapshot(snapshot), true);
			assert.deepStrictEqual(snapshot.permissions.map(entry => entry.status), ['denied', 'denied', 'not-determined', 'granted']);
		});
		test('a status query never asks for consent', async () => {
			await createPermissions('darwin').execute({ type: 'status' });
			assert.deepStrictEqual(preferences.prompts, []);
		});
		test('statuses are read live, not remembered', () => {
			const permissions = createPermissions('darwin');
			permissions.snapshot();
			preferences.states.screen = 'granted';
			preferences.trusted = true;
			assert.strictEqual(permission(permissions, 'screen').status, 'granted');
			assert.strictEqual(permission(permissions, 'accessibility').status, 'granted');
		});
		test('granted Accessibility cannot be requested again', () => {
			preferences.trusted = true;
			assert.strictEqual(permission(createPermissions('darwin'), 'accessibility').canRequest, false);
		});
		test('a camera request asks the operating system and reports its answer', async () => {
			const snapshot = await createPermissions('darwin').execute({ type: 'request', kind: 'camera' });
			assert.deepStrictEqual(preferences.prompts, ['camera']);
			assert.strictEqual(snapshot.permissions[2].status, 'granted');
		});
		test('a denied microphone cannot be requested, and asks nothing', async () => {
			preferences.states.microphone = 'denied';
			await assert.rejects(createPermissions('darwin').execute({ type: 'request', kind: 'microphone' }));
			assert.deepStrictEqual(preferences.prompts, []);
		});
		for (const [kind, page] of [['microphone', 'Privacy_Microphone'], ['screen', 'Privacy_ScreenCapture']] as const) {
			test(`the ${kind} settings open the fixed System Settings page, without a prompt`, async () => {
				await createPermissions('darwin').execute({ type: 'openSettings', kind });
				assert.deepStrictEqual(urls, [`x-apple.systempreferences:com.apple.preference.security?${page}`]);
				assert.deepStrictEqual(preferences.prompts, []);
			});
		}
		test('an Accessibility request prompts but does not assume the prompt grants access', async () => {
			const snapshot = await createPermissions('darwin').execute({ type: 'request', kind: 'accessibility' });
			assert.strictEqual(snapshot.permissions[1].status, 'denied');
			assert.deepStrictEqual(preferences.prompts, ['accessibility']);
		});
		test('Screen Recording can be requested before it is granted', () => {
			assert.strictEqual(permission(createPermissions('darwin'), 'screen').canRequest, true);
		});
		test('a Screen Recording request leaves the status to macOS and then points to its settings', async () => {
			const snapshot = await createPermissions('darwin').execute({ type: 'request', kind: 'screen' });
			assert.deepStrictEqual(preferences.prompts, ['screen']);
			assert.deepStrictEqual(snapshot.permissions[0], { kind: 'screen', status: 'denied', canRequest: false, canOpenSettings: true });
		});
		test('Screen Recording is asked for only once', async () => {
			const permissions = createPermissions('darwin');
			await permissions.execute({ type: 'request', kind: 'screen' });
			await assert.rejects(permissions.execute({ type: 'request', kind: 'screen' }));
			assert.deepStrictEqual(preferences.prompts, ['screen']);
		});
		test('granted Screen Recording cannot be requested', () => {
			preferences.states.screen = 'granted';
			assert.strictEqual(permission(createPermissions('darwin'), 'screen').canRequest, false);
		});
	});
	suite('Windows', () => {
		for (const kind of ['screen', 'accessibility'] as const) {
			test(`${kind} needs no consent, so there is nothing to request or open`, () => {
				assert.deepStrictEqual(permission(createPermissions('win32'), kind), { kind, status: 'not-required', canRequest: false, canOpenSettings: false });
			});
		}
		test('the camera cannot be requested from the app', async () => {
			const permissions = createPermissions('win32');
			assert.strictEqual(permission(permissions, 'camera').canRequest, false);
			await assert.rejects(permissions.execute({ type: 'request', kind: 'camera' }));
		});
		test('the camera settings open the fixed Windows privacy page', async () => {
			await createPermissions('win32').execute({ type: 'openSettings', kind: 'camera' });
			assert.deepStrictEqual(urls, ['ms-settings:privacy-webcam']);
		});
	});
	suite('Linux', () => {
		test('every permission is unsupported, with nothing to request or open', () => {
			assert.deepStrictEqual(createPermissions('linux').snapshot().permissions.filter(entry => entry.status !== 'unsupported' || entry.canRequest || entry.canOpenSettings), []);
		});
		test('opening settings is refused and asks nothing', async () => {
			await assert.rejects(createPermissions('linux').execute({ type: 'openSettings', kind: 'screen' }));
			assert.deepStrictEqual(preferences.prompts, []);
			assert.deepStrictEqual(urls, []);
		});
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

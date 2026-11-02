// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { settingDefaults, settingKeys, settingSchema, validateSetting } from '../../common/configurationSchema.js';

suite('configurationSchema', () => {
	for (const key of settingKeys()) {
		test(`${key} has a title and a valid default`, () => {
			const schema = settingSchema(key);
			assert.ok(schema?.title);
			assert.strictEqual(validateSetting(key, schema.default), undefined);
		});
	}
	const refused: readonly (readonly [string, string, unknown, RegExp])[] = [
		['a theme that is not offered', 'appearance.theme', 'sepia', /available options/],
		['an idle timeout above a day', 'power.incoming.idleTimeout', 1441, /0 to 1440/],
		['an idle timeout that is not whole seconds', 'power.incoming.idleTimeout', 1 / 7, /whole seconds/],
		['a fractional terminal limit', 'terminal.maxSessions', 2.5, /whole number/],
		['a desktop frame rate above 60 fps', 'desktop.quality', { max_bitrate_kbps: 1, max_frame_rate: 61, max_width: 1, max_height: 1 }, /60 fps/],
		['an unknown setting', 'future.setting', true, /Unknown setting/],
	];
	for (const [name, key, value, message] of refused) {
		test(`${name} is refused`, () => {
			assert.match(validateSetting(key, value) ?? '', message);
		});
	}
	test('an idle timeout in whole seconds is accepted', () => {
		assert.strictEqual(validateSetting('power.incoming.idleTimeout', 1.5), undefined);
	});
	test('automatic desktop quality is the default, its bandwidth limit the most a stream holds', () => {
		assert.deepStrictEqual(settingDefaults('user')['desktop.quality'], { max_bitrate_kbps: 50000, max_frame_rate: 30, max_width: 1920, max_height: 1080 });
	});
	test('the native background is not a setting', () => {
		assert.strictEqual(settingKeys().includes('appearance.nativeBackground'), false);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

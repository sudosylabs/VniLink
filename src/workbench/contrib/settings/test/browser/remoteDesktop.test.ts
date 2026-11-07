// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../../base/test/common/virtualScheduling/index.js';
import { click, edit, element, SettingsFixture, settle } from './settingsFixture.js';

suite('Remote desktop settings page', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: SettingsFixture;
	let page: HTMLElement;
	setup(() => {
		fixture = disposables.add(new SettingsFixture());
	});

	/** Opens the page; inside `runWithFakedTimers` its typed values wait in virtual time. */
	async function open(): Promise<void> {
		page = fixture.open('desktop').element;
		await settle();
	}
	const choice = (id: string): HTMLButtonElement => element<HTMLButtonElement>(page, `[data-bandwidth="${id}"]`);
	const pressed = (id: string): string | null => choice(id).getAttribute('aria-pressed');
	const bandwidth = (): HTMLInputElement => element<HTMLInputElement>(page, '[data-quality="max_bitrate_kbps"]');
	const frameRate = (): HTMLInputElement => element<HTMLInputElement>(page, '[data-quality="max_frame_rate"]');
	const codec = (): HTMLSelectElement => element<HTMLSelectElement>(page, 'select[data-setting="desktop.codec"]');
	test('the page shows its stream limits and nothing of a session or this computer\'s displays', async () => {
		await open();
		assert.ok(page.classList.contains('remote-desktop-page'));
		assert.ok(page.textContent.includes('Stream limits'));
		for (const text of ['Session overrides', 'Video processing on this computer', 'Displays']) {
			assert.ok(!page.textContent.includes(text), text);
		}
	});
	test('automatic bandwidth is the default, with no maximum to edit', async () => {
		await open();
		assert.strictEqual(pressed('automatic'), 'true');
		assert.strictEqual(bandwidth().disabled, true);
	});
	test('data saver applies its limit, shows as chosen and leaves no maximum to edit', () => runWithFakedTimers({}, async () => {
		await open();
		click(choice('dataSaver'));
		await fixture.settleEdit();
		assert.deepStrictEqual([pressed('dataSaver'), pressed('automatic')], ['true', 'false']);
		const quality = fixture.configuration.getValue<{ max_bitrate_kbps?: unknown } | null>('desktop.quality');
		assert.strictEqual(quality?.max_bitrate_kbps, 2000);
		assert.strictEqual(bandwidth().disabled, true);
	}));
	test('maximum offers a limit to edit', () => runWithFakedTimers({}, async () => {
		await open();
		click(choice('maximum'));
		await fixture.settleEdit();
		assert.strictEqual(pressed('maximum'), 'true');
		assert.strictEqual(bandwidth().disabled, false);
		assert.strictEqual(bandwidth().value, '20000');
	}));
	test('a typed maximum applies and stays chosen as Maximum', () => runWithFakedTimers({}, async () => {
		await open();
		click(choice('maximum'));
		await fixture.settleEdit();
		edit(bandwidth(), '8000');
		await fixture.settleEdit();
		assert.strictEqual(pressed('maximum'), 'true');
		assert.ok(JSON.stringify(fixture.configuration.getValue<unknown>('desktop.quality')).includes('"max_bitrate_kbps":8000'));
	}));
	test('an out-of-range frame rate is not applied', () => runWithFakedTimers({}, async () => {
		await open();
		edit(frameRate(), '61');
		await fixture.settleEdit();
		assert.strictEqual(fixture.configuration.updateCount(), 0);
	}));
	test('a valid frame rate is applied', () => runWithFakedTimers({}, async () => {
		await open();
		edit(frameRate(), '60');
		await fixture.settleEdit();
		assert.strictEqual(fixture.configuration.updateCount(), 1);
		assert.strictEqual(fixture.configuration.getValue<{ max_frame_rate?: unknown } | null>('desktop.quality')?.max_frame_rate, 60);
	}));
	test('the codec choice offers only what this computer decodes', async () => {
		await open();
		assert.strictEqual(codec().disabled, false);
		assert.deepStrictEqual(Array.from(codec().options).map(option => option.value), ['auto', 'h264', 'vp9', 'av1']);
	});
	test('a codec choice applies', async () => {
		await open();
		edit(codec(), 'vp9');
		await settle();
		assert.strictEqual(fixture.configuration.getValue<unknown>('desktop.codec'), 'vp9');
	});
	test('a saved codec this computer cannot decode is explained', async () => {
		await open();
		fixture.configuration.set('desktop.codec', 'h265');
		assert.ok(page.textContent.includes('Your saved codec isn’t available on this computer.'));
	});
});

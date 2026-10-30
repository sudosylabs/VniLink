// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { assert, edit, settle, settleEdit, SettingsFixture } from './settingsFixture.js';

/** Bandwidth, frame rate and size limits apply as defaults; codecs offer only what this computer decodes. */
export async function verifyRemoteDesktop(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		const page = fixture.open('desktop');
		await settle();
		assert(page.element.classList.contains('remote-desktop-page'), 'Remote desktop page class changed');
		assert(page.element.textContent.includes('Stream limits'), 'Stream limits missing');
		for (const text of ['Session overrides', 'Video processing on this computer', 'Displays']) {
			assert(!page.element.textContent.includes(text), `Remote desktop page shows ${text}`);
		}
		const choice = (id: string): HTMLButtonElement | null => page.element.querySelector<HTMLButtonElement>(`[data-bandwidth="${id}"]`);
		const bandwidth = page.element.querySelector<HTMLInputElement>('[data-quality="max_bitrate_kbps"]');
		assert(bandwidth, 'Maximum bandwidth missing');
		const editable = (): boolean => !bandwidth.disabled;
		assert(choice('automatic')?.getAttribute('aria-pressed') === 'true' && !editable(), 'Automatic is not the default, or leaves a maximum to edit');
		choice('dataSaver')?.click();
		await settleEdit();
		assert(choice('dataSaver')?.getAttribute('aria-pressed') === 'true' && choice('automatic')?.getAttribute('aria-pressed') === 'false', 'Data saver did not show as chosen');
		const quality = fixture.configuration.getValue<unknown>('desktop.quality');
		assert(typeof quality === 'object' && quality !== null && 'max_bitrate_kbps' in quality && quality.max_bitrate_kbps === 2000, 'Data saver did not apply');
		assert(!editable(), 'Data saver leaves a maximum to edit');
		choice('maximum')?.click();
		await settleEdit();
		assert(choice('maximum')?.getAttribute('aria-pressed') === 'true' && editable() && bandwidth.value === '20000', 'Maximum did not offer a limit to edit');
		edit(bandwidth, '8000');
		await settleEdit();
		assert(choice('maximum')?.getAttribute('aria-pressed') === 'true', 'A typed maximum was not kept as Maximum');
		assert(JSON.stringify(fixture.configuration.getValue<unknown>('desktop.quality')).includes('"max_bitrate_kbps":8000'), 'A maximum did not apply');
		const frameRate = page.element.querySelector<HTMLInputElement>('[data-quality="max_frame_rate"]');
		assert(frameRate, 'Frame rate missing');
		const updates = fixture.configuration.updateCount();
		edit(frameRate, '61');
		await settleEdit();
		assert(fixture.configuration.updateCount() === updates, 'An out-of-range frame rate was applied');
		edit(frameRate, '60');
		await settleEdit();
		assert(fixture.configuration.updateCount() === updates + 1, 'A valid frame rate was not applied');
		const codec = page.element.querySelector<HTMLSelectElement>('select[data-setting="desktop.codec"]');
		assert(codec && !codec.disabled, 'Codec choice missing');
		assert(Array.from(codec.options).map(option => option.value).join() === 'auto,h264,vp9,av1', 'Codec options were not limited to decodable codecs');
		edit(codec, 'vp9');
		await settle();
		assert(fixture.configuration.getValue<unknown>('desktop.codec') === 'vp9', 'Codec did not apply');
		fixture.configuration.set('desktop.codec', 'h265');
		assert(page.element.textContent.includes('Your saved codec isn’t available on this computer.'), 'Unavailable saved codec was not explained');
	} finally {
		fixture.dispose();
	}
}

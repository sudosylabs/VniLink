import { assert, edit, settle, settleEdit, SettingsFixture } from './settingsFixture.js';

/** Quality presets and limits apply as defaults; codecs offer only what this computer decodes. */
export async function verifyRemoteDesktop(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		const page = fixture.open('desktop');
		await settle();
		assert(page.element.classList.contains('remote-desktop-page'), 'Remote desktop page class changed');
		assert(page.element.textContent.includes('Default stream quality'), 'Quality defaults missing');
		for (const text of ['Session overrides', 'Video processing on this computer', 'Displays']) {
			assert(!page.element.textContent.includes(text), `Remote desktop page shows ${text}`);
		}
		page.element.querySelector<HTMLButtonElement>('[data-preset="balanced"]')?.click();
		assert(page.element.querySelector('[data-preset="balanced"]')?.getAttribute('aria-pressed') === 'true', 'Preset did not show as chosen');
		await settleEdit();
		const quality = fixture.configuration.getValue<unknown>('desktop.quality');
		assert(typeof quality === 'object' && quality !== null && 'max_bitrate_kbps' in quality && quality.max_bitrate_kbps === 5000, 'Preset did not apply');
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
		assert(page.element.textContent.includes('Your saved codec is unavailable on this computer.'), 'Unavailable saved codec was not explained');
	} finally {
		fixture.dispose();
	}
}

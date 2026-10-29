import { assert, edit, settle, SettingsFixture } from './settingsFixture.js';

/** Media sources come from this computer's devices; missing system access is offered as an action. */
export async function verifyMediaSettings(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		const page = fixture.open('media');
		await settle();
		assert(page.element.querySelectorAll('.settings-select > .vnilink-icon').length === 3, 'Media source pickers missing');
		const cameras = page.element.querySelector<HTMLSelectElement>('select[data-setting="media.camera"]');
		assert(cameras, 'Camera picker missing');
		assert(Array.from(cameras.options).map(option => option.value).join() === 'vnilink:all-cameras,,camera-one', 'Camera options changed');
		assert(Array.from(cameras.options).some(option => option.textContent === 'Camera <one>'), 'Device name was not shown as text');
		edit(cameras, 'camera-one');
		await settle();
		assert(fixture.configuration.getValue<unknown>('media.camera') === 'camera-one', 'Camera choice did not apply');
		fixture.configuration.set('media.microphone', 'gone');
		assert(page.element.querySelector<HTMLSelectElement>('select[data-setting="media.microphone"]')?.selectedOptions[0]?.textContent === 'Selected device (unavailable)', 'Missing microphone was not shown');
		// Access the camera still needs is asked for in its own row, not above the page.
		const cameraRow = cameras.closest('.setting-row');
		const request = cameraRow?.querySelector<HTMLButtonElement>('button.settings-button');
		assert(request?.hidden === false && request.textContent === 'Allow', 'Camera access action missing from its row');
		assert(cameraRow?.textContent.includes('Camera access isn’t allowed yet.') === true, 'Missing camera access is not explained');
		request.click();
		await settle();
		assert(fixture.permissions.commands.some(command => command.type === 'request' && command.kind === 'camera'), 'Camera access was not requested');
		// Read again: the request re-rendered the row.
		const offered = (): boolean => cameras.closest('.setting-row')?.querySelector<HTMLButtonElement>('button.settings-button')?.hidden === false;
		const explained = (): boolean => cameras.closest('.setting-row')?.textContent.includes('isn’t allowed') === true;
		assert(!offered() && !explained(), 'Granted camera access was still offered');
	} finally {
		fixture.dispose();
	}
}

import { assert, edit, settle, SettingsFixture } from './settingsFixture.js';

/** Media sources come from this computer's devices; missing system access is offered as an action. */
export async function verifyMediaSettings(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		const page = fixture.open('media');
		await settle();
		assert(page.element.querySelectorAll('.settings-select > .vnilink-icon').length === 3, 'Media source pickers missing');
		const cameras = page.element.querySelector<HTMLSelectElement>('select[aria-label="Cameras"]');
		assert(cameras, 'Camera picker missing');
		assert(Array.from(cameras.options).map(option => option.value).join() === 'vnilink:auto-camera,,camera-one', 'Camera options changed');
		assert(Array.from(cameras.options).some(option => option.textContent === 'Camera <one>'), 'Device name was not shown as text');
		edit(cameras, 'camera-one');
		await settle();
		assert(fixture.configuration.getValue<unknown>('media.camera') === 'camera-one', 'Camera choice did not apply');
		fixture.configuration.set('media.microphone', 'gone');
		assert(page.element.querySelector<HTMLSelectElement>('select[aria-label="Microphones"]')?.selectedOptions[0]?.textContent === 'Selected device (unavailable)', 'Missing microphone was not shown');
		const request = Array.from(page.element.querySelectorAll<HTMLButtonElement>('.settings-apply-row button')).find(button => button.textContent === 'Allow camera access');
		assert(request, 'Camera access action missing');
		request.click();
		await settle();
		assert(fixture.permissions.commands.some(command => command.type === 'request' && command.kind === 'camera'), 'Camera access was not requested');
		assert(!Array.from(page.element.querySelectorAll('.settings-apply-row button')).some(button => button.textContent === 'Allow camera access'), 'Granted camera access was still offered');
	} finally {
		fixture.dispose();
	}
}

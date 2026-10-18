import { assert, settle, SettingsFixture, toggle } from './settingsFixture.js';

/** Sharing and remote action rules apply per switch; OS access is live state with its own actions. */
export async function verifyAccess(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		fixture.configuration.setRestricted('security.allowRestart', true);
		const page = fixture.open('access');
		await settle();
		const view = page.element.querySelector<HTMLInputElement>('[data-setting="sharing.screen"]');
		assert(view && !view.disabled && !view.checked, 'Screen sharing switch missing');
		toggle(view);
		await settle();
		assert(fixture.configuration.updates.at(-1)?.key === 'sharing.screen' && view.checked, 'Screen sharing did not apply');
		fixture.configuration.failure = new Error('Sharing settings could not be changed.');
		toggle(view);
		await settle();
		assert(view.checked, 'Refused sharing change kept the refused value');
		assert(view.closest('.access-section')?.querySelector('.access-status')?.textContent === 'Sharing settings could not be changed.', 'Refused sharing change was not explained in its section');
		fixture.configuration.failure = undefined;
		const restart = page.element.querySelector<HTMLInputElement>('[data-setting="security.allowRestart"]');
		assert(restart?.disabled && restart.closest('.access-toggle-row')?.textContent.includes('Not supported on this computer.'), 'Unsupported action was not marked');
		assert(page.element.querySelector<HTMLInputElement>('[data-setting="security.allowLock"]')?.checked, 'Remote lock default missing');
		const permissions = page.element.querySelector('[data-permission-kind="camera"]')?.closest('.access-section');
		assert(permissions && !permissions.querySelector('input'), 'Permissions section shows a switch');
		assert(!permissions.textContent.includes('Development builds'), 'Permissions retained development-only copy');
		const camera = permissions.querySelector<HTMLButtonElement>('[data-permission-kind="camera"] button');
		assert(camera?.textContent === 'Request access', 'Camera request missing');
		assert(permissions.querySelector('[data-permission-kind="screen"] button')?.textContent === 'Open settings', 'Screen settings action missing');
		camera.click();
		await settle();
		assert(permissions.querySelector('[data-permission-kind="camera"] [data-status="granted"]'), 'Camera request did not refresh');
		fixture.permissions.fail = true;
		fixture.permissions.focused.fire();
		await settle();
		assert(fixture.configuration.reloads > 0, 'Focus did not read the engine settings again');
		assert(permissions.querySelector<HTMLButtonElement>('button')?.disabled, 'Failed check left actions enabled');
		const commands = fixture.permissions.commands.length;
		page.dispose();
		fixture.permissions.focused.fire();
		await settle();
		assert(fixture.permissions.commands.length === commands, 'Disposed page kept checking OS access');
	} finally {
		fixture.dispose();
	}
}

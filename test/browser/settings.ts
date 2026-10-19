import { assert, current, edit, settle, settleEdit, SettingsFixture, toggle } from './settingsFixture.js';

const pageIds = ['appearance', 'connection', 'access', 'desktop', 'input', 'terminal', 'media', 'recording', 'power'];

/** Every page renders from its layout and applies changes without a Save button. */
async function verifyPagesRender(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		for (const id of pageIds) {
			const page = fixture.open(id);
			await settle();
			assert(page.element.dataset.settingsPage === id, `${id} page lost its identity`);
			assert(page.element.querySelector('h1')?.textContent, `${id} page has no heading`);
			assert(!page.element.querySelector('button[type="submit"], form'), `${id} page still saves through a form`);
		}
	} finally {
		fixture.dispose();
	}
}

/** Switches apply at once; a refused switch shows the value in effect and why. */
async function verifySwitches(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		const page = fixture.open('power');
		const lock = page.element.querySelector<HTMLInputElement>('[data-setting="power.lockAfterControl"]');
		assert(lock?.getAttribute('role') === 'switch' && !lock.disabled, 'Power switch missing or disabled');
		toggle(lock);
		await settle();
		assert(fixture.configuration.updates.at(-1)?.key === 'power.lockAfterControl' && fixture.configuration.updates.at(-1)?.value === true, 'Switch did not apply at once');
		assert(lock.checked, 'Applied switch did not show its value');
		fixture.configuration.failure = new Error('Engine refused');
		toggle(lock);
		await settle();
		assert(lock.checked, 'Refused switch kept the refused value');
		assert(page.element.querySelector('.settings-status')?.textContent === 'Engine refused', 'Refused switch did not explain why');
		fixture.configuration.failure = undefined;
		toggle(lock);
		await settle();
		assert(page.element.querySelector('.settings-status')?.textContent === '', 'A successful change kept the previous error');
	} finally {
		fixture.dispose();
	}
}

/** Typed numbers wait for a pause, are validated against the schema, and stay for correction when refused. */
async function verifyNumbers(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		const page = fixture.open('power');
		const idle = page.element.querySelector<HTMLInputElement>('[data-setting="power.incoming.idleTimeout"]');
		assert(idle?.type === 'number' && idle.min === '0' && idle.max === '1440', 'Idle timeout bounds missing');
		assert(idle.nextElementSibling?.textContent === 'min', 'Idle timeout unit missing');
		edit(idle, '1');
		edit(idle, '15');
		await settle();
		assert(fixture.configuration.updateCount() === 0, 'A typed number applied before the user paused');
		await settleEdit();
		assert(fixture.configuration.updateCount() === 1 && fixture.configuration.updates[0].value === 15, 'Typed number was not applied once after the pause');
		edit(idle, '2000');
		await settleEdit();
		assert(fixture.configuration.updateCount() === 1, 'An out-of-range number was applied');
		assert(page.element.querySelector('.settings-status')?.textContent.includes('0 to 1440'), 'Out-of-range number was not explained');
		fixture.configuration.failure = new Error('Disk unavailable');
		edit(idle, '30');
		await settleEdit();
		assert(idle.value === '30' && page.element.querySelector('.settings-status')?.textContent === 'Disk unavailable', 'Refused number was discarded or not explained');
		fixture.configuration.set('power.incoming.idleTimeout', 45);
		assert(current(idle).value === '30', 'A refreshed value replaced the edit awaiting correction');
	} finally {
		fixture.dispose();
	}
}

/** Engine settings are disabled with the reason while their source cannot be read, and read again on focus. */
async function verifyUnavailable(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		fixture.configuration.setError('terminal', 'The connection engine is unavailable. Restart VniLink to try again.');
		const page = fixture.open('terminal');
		await settle();
		const total = page.element.querySelector<HTMLInputElement>('[data-setting="terminal.maxSessions"]');
		assert(total?.disabled, 'Unavailable terminal limit stayed editable');
		assert(page.element.querySelector('.settings-status')?.textContent.includes('unavailable'), 'Unavailable reason missing');
		// No Refresh button: the page reads the engine again when the window gains focus.
		assert(![...page.element.querySelectorAll('button')].some(button => button.textContent === 'Refresh'), 'A Refresh button is offered');
		const reloads = fixture.configuration.reloads;
		fixture.permissions.focused.fire();
		await settle();
		assert(fixture.configuration.reloads === reloads + 1, 'Gaining focus did not read the engine settings again');
		fixture.configuration.setError('terminal', undefined);
		assert(!current(total).disabled && page.element.querySelector('.settings-status')?.textContent === '', 'Readable terminal limit stayed disabled');
	} finally {
		fixture.dispose();
	}
}

/** A choice, the theme picker, the slider and the folder picker each apply their setting. */
async function verifyCustomControls(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		const appearance = fixture.open('appearance');
		assert(appearance.element.querySelectorAll('.theme-mode-picker .theme-choice').length === 3, 'Theme choices missing');
		assert(!appearance.element.textContent.includes('Translucent background'), 'Retired background setting is shown');
		const dark = appearance.element.querySelector<HTMLInputElement>('input[type="radio"][value="dark"]');
		assert(dark, 'Dark theme missing');
		dark.checked = true;
		dark.dispatchEvent(new Event('change'));
		await settle();
		assert(fixture.configuration.getValue<unknown>('appearance.theme') === 'dark', 'Theme did not apply');
		fixture.configuration.set('appearance.theme', 'light');
		assert(appearance.element.querySelector<HTMLInputElement>('input[type="radio"]:checked')?.value === 'light', 'Theme picker ignored an outside change');
		const input = fixture.open('input');
		const mode = input.element.querySelector<HTMLSelectElement>('select[data-setting="input.keyboardMode"]');
		assert(mode && Array.from(mode.options).map(option => option.value).join() === 'Character,Map,Translate', 'Keyboard mapping options changed');
		edit(mode, 'Map');
		await settle();
		assert(fixture.configuration.getValue<unknown>('input.keyboardMode') === 'Map', 'Keyboard mapping did not apply');
		const sensitivity = input.element.querySelector<HTMLInputElement>('input[type="range"]');
		assert(sensitivity, 'Pointer sensitivity missing');
		edit(sensitivity, '150');
		assert(input.element.querySelector('.setting-slider + output')?.textContent === '150%', 'Sensitivity did not show its value');
		await settleEdit();
		assert(fixture.configuration.getValue<unknown>('input.pointerSensitivity') === 150, 'Sensitivity did not apply');
		const recording = fixture.open('recording');
		await settle();
		assert(recording.element.querySelector('.recording-path')?.textContent === '/Pictures/VniLink', 'Default recording folder not shown');
		recording.element.querySelector<HTMLButtonElement>('[data-section="destination"] .settings-button')?.click();
		await settle();
		assert(fixture.configuration.getValue<unknown>('recording.directory') === '/Recordings' && recording.element.querySelector('.recording-path')?.textContent === '/Recordings', 'Chosen folder did not apply');
		// Use default appears only once a folder is chosen.
		const reset = recording.element.querySelector<HTMLButtonElement>('[data-section="destination"] .settings-text-button');
		assert(reset?.hidden === false, 'Use default is not offered for a chosen folder');
		reset.click();
		await settle();
		assert(fixture.configuration.getValue<unknown>('recording.directory') === '', 'Default folder did not apply');
		const size = recording.element.querySelector<HTMLInputElement>('[data-setting="recording.maxSegmentSize"]');
		assert(size?.value === '1' && size.nextElementSibling?.textContent === 'GiB', 'File size not shown in GiB');
	} finally {
		fixture.dispose();
	}
}

/** The Terminal page offers this computer's shells; a choice applies, a refusal is explained, a missing one stays visible. */
async function verifyTerminalShell(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		const page = fixture.open('terminal');
		await settle();
		const select = page.element.querySelector<HTMLSelectElement>('select[data-setting="terminal.shell"]');
		assert(select, 'Shell choice missing');
		const options = (): string => Array.from(select.options).map(option => `${option.value}=${option.textContent}`).join();
		assert(options() === '=Automatic (zsh),/bin/zsh=zsh,/bin/bash=bash', `Shell options are wrong: ${options()}`);
		assert(current(select).value === '' && !current(select).disabled, 'Automatic is not the default choice');
		// Reading the list again must not rebuild it: that closes an open list, which looked like blinking.
		const first = select.options[0];
		fixture.permissions.focused.fire();
		await settle();
		assert(select.options[0] === first, 'Rereading unchanged shells rebuilt the list');
		assert(page.element.querySelector('section[data-section]')?.getAttribute('data-section') === 'shell', 'Shell is not the first section');
		edit(select, '/bin/bash');
		await settle();
		assert(fixture.configuration.getValue<unknown>('terminal.shell') === '/bin/bash', 'The chosen shell did not apply');
		fixture.configuration.failure = new Error('Close hosted terminals before changing the shell.');
		edit(select, '/bin/zsh');
		await settle();
		assert(page.element.textContent.includes('Close hosted terminals before changing the shell.'), 'A refused change is not explained');
		assert(fixture.configuration.getValue<unknown>('terminal.shell') === '/bin/bash', 'A refused change was kept');
		fixture.configuration.failure = undefined;
		// A saved shell that was uninstalled stays chosen and is called out.
		fixture.configuration.set('terminal.shell', '/opt/homebrew/bin/fish');
		await settle();
		assert(current(select).value === '/opt/homebrew/bin/fish' && options().includes('/opt/homebrew/bin/fish=/opt/homebrew/bin/fish (not installed)'), 'A missing saved shell is hidden');
		assert(page.element.textContent.includes('paired computers get the automatic shell'), 'A missing saved shell is not explained');
		// Installing it makes it an ordinary choice the next time the list is read.
		fixture.shells = { ...fixture.shells, shells: [...fixture.shells.shells, { id: '/opt/homebrew/bin/fish', label: 'fish' }] };
		fixture.permissions.focused.fire();
		await settle();
		assert(options().endsWith('/opt/homebrew/bin/fish=fish') && !page.element.textContent.includes('(not installed)'), 'A newly installed shell did not appear');
	} finally {
		fixture.dispose();
	}
}

/** A disposed page applies nothing. */
async function verifyDisposal(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		const page = fixture.open('power');
		const idle = page.element.querySelector<HTMLInputElement>('[data-setting="power.outgoing.idleTimeout"]');
		assert(idle, 'Idle timeout missing');
		edit(idle, '5');
		page.dispose();
		await settleEdit();
		assert(fixture.configuration.updateCount() === 0, 'Disposed page applied a pending edit');
		assert(!page.element.isConnected, 'Disposed page stayed in the document');
	} finally {
		fixture.dispose();
	}
}

export async function verifySettingsPages(): Promise<void> {
	await verifyPagesRender();
	await verifySwitches();
	await verifyNumbers();
	await verifyUnavailable();
	await verifyCustomControls();
	await verifyTerminalShell();
	await verifyDisposal();
}

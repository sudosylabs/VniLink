// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../../base/test/common/virtualScheduling/index.js';
import { click, current, edit, element, SettingsFixture, settle, toggle } from './settingsFixture.js';

const pageIds = ['general', 'connection', 'access', 'desktop', 'input', 'terminal', 'media', 'recording', 'power'];

suite('Settings pages', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: SettingsFixture;
	setup(() => {
		fixture = disposables.add(new SettingsFixture());
	});
	const status = (page: HTMLElement): string | undefined => page.querySelector('.settings-status')?.textContent;
	for (const id of pageIds) {
		test(`the ${id} page renders with its identity and a heading, and saves without a form`, async () => {
			const page = fixture.open(id).element;
			await settle();
			assert.strictEqual(page.dataset.settingsPage, id);
			assert.ok(page.querySelector('h1')?.textContent);
			assert.strictEqual(page.querySelector('button[type="submit"], form'), null);
		});
	}
	suite('switches', () => {
		let page: HTMLElement;
		let lock: HTMLInputElement;
		setup(() => {
			page = fixture.open('power').element;
			lock = element<HTMLInputElement>(page, '[data-setting="power.lockAfterControl"]');
		});
		test('a switch applies its setting at once and shows the value', async () => {
			assert.strictEqual(lock.getAttribute('role'), 'switch');
			assert.strictEqual(lock.disabled, false);
			toggle(lock);
			await settle();
			assert.deepStrictEqual(fixture.configuration.updates.at(-1), { key: 'power.lockAfterControl', value: true });
			assert.strictEqual(lock.checked, true);
		});
		test('a refused switch shows the value in effect and explains why', async () => {
			toggle(lock);
			await settle();
			fixture.configuration.failure = new Error('Engine refused');
			toggle(lock);
			await settle();
			assert.strictEqual(lock.checked, true);
			assert.strictEqual(status(page), 'Engine refused');
		});
		test('a successful change clears the previous error', async () => {
			fixture.configuration.failure = new Error('Engine refused');
			toggle(lock);
			await settle();
			fixture.configuration.failure = undefined;
			toggle(lock);
			await settle();
			assert.strictEqual(status(page), '');
		});
	});
	suite('numbers', () => {
		const idleField = (page: HTMLElement): HTMLInputElement => element<HTMLInputElement>(page, '[data-setting="power.incoming.idleTimeout"]');
		test('a number field shows its schema bounds and its unit', () => {
			const idle = idleField(fixture.open('power').element);
			assert.deepStrictEqual({ type: idle.type, min: idle.min, max: idle.max, unit: idle.nextElementSibling?.textContent }, { type: 'number', min: '0', max: '1440', unit: 'min' });
		});
		test('a typed number applies once, after the user pauses', () => runWithFakedTimers({}, async () => {
			const idle = idleField(fixture.open('power').element);
			edit(idle, '1');
			edit(idle, '15');
			await settle();
			assert.strictEqual(fixture.configuration.updateCount(), 0, 'applied before the pause');
			await fixture.settleEdit();
			assert.deepStrictEqual(fixture.configuration.updates, [{ key: 'power.incoming.idleTimeout', value: 15 }]);
		}));
		test('an out-of-range number is not applied, and the range is explained', () => runWithFakedTimers({}, async () => {
			const page = fixture.open('power').element;
			edit(idleField(page), '2000');
			await fixture.settleEdit();
			assert.strictEqual(fixture.configuration.updateCount(), 0);
			assert.ok(status(page)?.includes('0 to 1440'), status(page));
		}));
		test('a refused number stays for correction, with the reason', () => runWithFakedTimers({}, async () => {
			const page = fixture.open('power').element;
			const idle = idleField(page);
			fixture.configuration.failure = new Error('Disk unavailable');
			edit(idle, '30');
			await fixture.settleEdit();
			assert.strictEqual(idle.value, '30');
			assert.strictEqual(status(page), 'Disk unavailable');
		}));
		test('a value changed elsewhere does not replace an edit awaiting correction', () => runWithFakedTimers({}, async () => {
			const idle = idleField(fixture.open('power').element);
			fixture.configuration.failure = new Error('Disk unavailable');
			edit(idle, '30');
			await fixture.settleEdit();
			fixture.configuration.set('power.incoming.idleTimeout', 45);
			assert.strictEqual(current(idle).value, '30');
		}));
	});
	suite('a setting whose source cannot be read', () => {
		let page: HTMLElement;
		let total: HTMLInputElement;
		setup(async () => {
			fixture.configuration.setError('terminal', 'The connection engine is unavailable. Restart VniLink to try again.');
			page = fixture.open('terminal').element;
			await settle();
			total = element<HTMLInputElement>(page, '[data-setting="terminal.maxSessions"]');
		});
		test('is disabled, with the reason', () => {
			assert.strictEqual(total.disabled, true);
			assert.ok(status(page)?.includes('unavailable'), status(page));
		});
		test('is read again when the window gains focus, without a Refresh button', async () => {
			assert.ok(![...page.querySelectorAll('button')].some(button => button.textContent === 'Refresh'), 'a Refresh button is offered');
			const reloads = fixture.configuration.reloads;
			fixture.permissions.focus();
			await settle();
			// The page also reads again when the real window's visibility changes, which the test cannot hold still.
			assert.ok(fixture.configuration.reloads > reloads, 'focus did not read the settings again');
		});
		test('becomes editable, without the reason, once it can be read', () => {
			fixture.configuration.setError('terminal', undefined);
			assert.strictEqual(current(total).disabled, false);
			assert.strictEqual(status(page), '');
		});
	});
	suite('appearance and input', () => {
		test('General offers three theme choices and no retired background setting', () => {
			const page = fixture.open('general').element;
			assert.strictEqual(page.querySelectorAll('[data-section="appearance"] .theme-mode-picker .theme-choice').length, 3);
			assert.ok(!page.textContent.includes('Translucent background'));
		});
		test('choosing a theme applies it', async () => {
			const dark = element<HTMLInputElement>(fixture.open('general').element, 'input[type="radio"][value="dark"]');
			dark.checked = true;
			dark.dispatchEvent(new Event('change'));
			await settle();
			assert.strictEqual(fixture.configuration.getValue<unknown>('appearance.theme'), 'dark');
		});
		test('the theme picker follows a change made elsewhere', () => {
			const page = fixture.open('general').element;
			fixture.configuration.set('appearance.theme', 'dark');
			fixture.configuration.set('appearance.theme', 'light');
			assert.strictEqual(page.querySelector<HTMLInputElement>('input[type="radio"]:checked')?.value, 'light');
		});
		test('the keyboard mapping offers its three modes and applies a choice', async () => {
			const mode = element<HTMLSelectElement>(fixture.open('input').element, 'select[data-setting="input.keyboardMode"]');
			assert.deepStrictEqual(Array.from(mode.options).map(option => option.value), ['Character', 'Map', 'Translate']);
			edit(mode, 'Map');
			await settle();
			assert.strictEqual(fixture.configuration.getValue<unknown>('input.keyboardMode'), 'Map');
		});
		test('the pointer sensitivity slider shows its value at once and applies it after a pause', () => runWithFakedTimers({}, async () => {
			const page = fixture.open('input').element;
			edit(element<HTMLInputElement>(page, 'input[type="range"]'), '150');
			assert.strictEqual(page.querySelector('.setting-slider + output')?.textContent, '150%');
			await fixture.settleEdit();
			assert.strictEqual(fixture.configuration.getValue<unknown>('input.pointerSensitivity'), 150);
		}));
	});
	suite('recording', () => {
		let page: HTMLElement;
		setup(async () => {
			page = fixture.open('recording').element;
			await settle();
		});
		const path = (): string | undefined => page.querySelector('.recording-path')?.textContent;
		const reset = (): HTMLButtonElement => element<HTMLButtonElement>(page, '[data-section="destination"] .settings-text-button');
		test('the default folder is shown', () => {
			assert.strictEqual(path(), '/Pictures/VniLink');
		});
		test('a chosen folder applies and is shown', async () => {
			click(element<HTMLButtonElement>(page, '[data-section="destination"] .settings-button'));
			await settle();
			assert.strictEqual(fixture.configuration.getValue<unknown>('recording.directory'), '/Recordings');
			assert.strictEqual(path(), '/Recordings');
		});
		test('Use default is offered for a chosen folder and restores the default', async () => {
			click(element<HTMLButtonElement>(page, '[data-section="destination"] .settings-button'));
			await settle();
			assert.strictEqual(reset().hidden, false);
			click(reset());
			await settle();
			assert.strictEqual(fixture.configuration.getValue<unknown>('recording.directory'), '');
		});
		test('the file size is shown in GiB', () => {
			const size = element<HTMLInputElement>(page, '[data-setting="recording.maxSegmentSize"]');
			assert.strictEqual(size.value, '1');
			assert.strictEqual(size.nextElementSibling?.textContent, 'GiB');
		});
	});
	suite('terminal shell', () => {
		let page: HTMLElement;
		let select: HTMLSelectElement;
		setup(async () => {
			page = fixture.open('terminal').element;
			await settle();
			select = element<HTMLSelectElement>(page, 'select[data-setting="terminal.shell"]');
		});
		const options = (): string[] => Array.from(select.options).map(option => `${option.value}=${option.textContent}`);
		test('offers automatic and this computer\'s shells, automatic by default', () => {
			assert.deepStrictEqual(options(), ['=Automatic (zsh)', '/bin/zsh=zsh', '/bin/bash=bash']);
			assert.deepStrictEqual(current(select), { value: '', disabled: false });
		});
		test('reading unchanged shells again does not rebuild the list', async () => {
			// Rebuilding closes an open list, which looks like blinking.
			const first = select.options[0];
			fixture.permissions.focus();
			await settle();
			assert.strictEqual(select.options[0], first);
		});
		test('is the first section of the Terminal page', () => {
			assert.strictEqual(page.querySelector('section[data-section]')?.getAttribute('data-section'), 'shell');
		});
		test('a chosen shell applies', async () => {
			edit(select, '/bin/bash');
			await settle();
			assert.strictEqual(fixture.configuration.getValue<unknown>('terminal.shell'), '/bin/bash');
		});
		test('a refused change is explained and not kept', async () => {
			edit(select, '/bin/bash');
			await settle();
			fixture.configuration.failure = new Error('Close hosted terminals before changing the shell.');
			edit(select, '/bin/zsh');
			await settle();
			assert.ok(page.textContent.includes('Close hosted terminals before changing the shell.'));
			assert.strictEqual(fixture.configuration.getValue<unknown>('terminal.shell'), '/bin/bash');
		});
		test('a saved shell that is not installed stays chosen and is explained', async () => {
			fixture.configuration.set('terminal.shell', '/opt/homebrew/bin/fish');
			await settle();
			assert.strictEqual(current(select).value, '/opt/homebrew/bin/fish');
			assert.ok(options().includes('/opt/homebrew/bin/fish=/opt/homebrew/bin/fish (not installed)'), options().join());
			assert.ok(page.textContent.includes('paired computers get the automatic shell'));
		});
		test('a newly installed shell becomes an ordinary choice when the list is read again', async () => {
			fixture.configuration.set('terminal.shell', '/opt/homebrew/bin/fish');
			await settle();
			fixture.shells = { ...fixture.shells, shells: [...fixture.shells.shells, { id: '/opt/homebrew/bin/fish', label: 'fish' }] };
			fixture.permissions.focus();
			await settle();
			assert.strictEqual(options().at(-1), '/opt/homebrew/bin/fish=fish');
			assert.ok(!page.textContent.includes('(not installed)'));
		});
	});
	test('a disposed page applies no pending edit and leaves the document', () => runWithFakedTimers({}, async () => {
		const page = fixture.open('power');
		edit(element<HTMLInputElement>(page.element, '[data-setting="power.outgoing.idleTimeout"]'), '5');
		page.dispose();
		await fixture.settleEdit();
		assert.strictEqual(fixture.configuration.updateCount(), 0);
		assert.strictEqual(page.element.isConnected, false);
	}));
});

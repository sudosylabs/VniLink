// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { assert, settle, SettingsFixture } from './settingsFixture.js';

function change(input: HTMLInputElement, value?: string): void {
	if (value !== undefined) {
		input.value = value;
	}
	input.dispatchEvent(new Event('change'));
}

/** Relays are edited as a draft and committed whole once valid; saved tokens never come back. */
export async function verifyConnection(): Promise<void> {
	const fixture = new SettingsFixture();
	try {
		const page = fixture.open('connection');
		await settle();
		assert(page.element.querySelector('a')?.href === 'https://docs.iroh.computer/relays/rate-limiting', 'Public relay documentation missing');
		page.element.querySelector('a')?.click();
		assert(fixture.openedDocumentation === 1, 'Relay documentation did not open externally');
		assert(page.element.querySelector('.connection-active-description')?.textContent.includes('public'), 'Running configuration missing');
		const custom = page.element.querySelector<HTMLInputElement>('input[type="radio"][value="custom"]');
		assert(custom, 'Relay mode missing');
		custom.checked = true;
		change(custom);
		await settle();
		assert(fixture.configuration.updateCount() === 0, 'An empty custom relay list was committed');
		const url = page.element.querySelector<HTMLInputElement>('.relay-editor input[type="text"]');
		const auth = page.element.querySelector<HTMLInputElement>('.relay-editor input[type="checkbox"]');
		const token = page.element.querySelector<HTMLInputElement>('.relay-editor input[type="password"]');
		assert(url && auth && token, 'Custom relay controls missing');
		change(url, 'http://remote.example/');
		await settle();
		assert(fixture.configuration.updateCount() === 0 && url.getAttribute('aria-invalid') === 'true', 'An invalid relay was committed or not marked');
		change(url, 'https://relay.example/');
		await settle();
		assert(fixture.configuration.updateCount() === 1, 'A valid relay list was not committed');
		assert(url.getAttribute('aria-invalid') === 'false', 'A corrected relay stayed marked invalid');
		auth.checked = true;
		change(auth);
		await settle();
		assert(fixture.configuration.updateCount() === 1 && token.getAttribute('aria-invalid') === 'true', 'An authenticated relay without a token was committed');
		change(token, 'secret-token');
		await settle();
		const committed = fixture.configuration.updates.at(-1)?.value;
		assert(Array.isArray(committed) && JSON.stringify(committed) === JSON.stringify([{ url: 'https://relay.example/', authenticated: true, replacementToken: 'secret-token' }]), 'Token was not committed with its relay');
		assert(token.value === '' && !JSON.stringify(fixture.configuration.getValue<unknown>('connection.relays')).includes('secret-token'), 'A saved token stayed visible');
		assert(page.element.querySelector('.relay-editor')?.textContent.includes('Leave blank to keep the saved token'), 'Saved token was not offered for reuse');
		const publicMode = page.element.querySelector<HTMLInputElement>('input[type="radio"][value="public"]');
		assert(publicMode, 'Public mode missing');
		publicMode.checked = true;
		change(publicMode);
		await settle();
		assert(JSON.stringify(fixture.configuration.getValue<unknown>('connection.relays')) === '[]', 'Public relays were not committed');
		fixture.configuration.setError('relays', 'Connection settings are unavailable because the connection engine could not start. Restart VniLink to try again.');
		assert(publicMode.matches(':disabled') && page.element.querySelector('.settings-status')?.textContent.includes('unavailable'), 'Unavailable relays stayed editable');
	} finally {
		fixture.dispose();
	}
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { click, element, SettingsFixture, settle } from './settingsFixture.js';

/** Commits `input`, after setting its text to `value` when one is given, as leaving the field does. */
function change(input: HTMLInputElement, value?: string): void {
	if (value !== undefined) {
		input.value = value;
	}
	input.dispatchEvent(new Event('change'));
}

suite('Connection settings page', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: SettingsFixture;
	let page: HTMLElement;
	setup(async () => {
		fixture = disposables.add(new SettingsFixture());
		page = fixture.open('connection').element;
		await settle();
	});

	/** Chooses custom relays and returns the editor's address, authentication and token fields. */
	async function custom(): Promise<{ url: HTMLInputElement; auth: HTMLInputElement; token: HTMLInputElement }> {
		const mode = element<HTMLInputElement>(page, 'input[type="radio"][value="custom"]');
		mode.checked = true;
		change(mode);
		await settle();
		return {
			url: element<HTMLInputElement>(page, '.relay-editor input[type="text"]'),
			auth: element<HTMLInputElement>(page, '.relay-editor input[type="checkbox"]'),
			token: element<HTMLInputElement>(page, '.relay-editor input[type="password"]'),
		};
	}

	/** Commits one authenticated custom relay with a token. */
	async function authenticatedRelay(): Promise<HTMLInputElement> {
		const { url, auth, token } = await custom();
		change(url, 'https://relay.example/');
		await settle();
		auth.checked = true;
		change(auth);
		await settle();
		change(token, 'secret-token');
		await settle();
		return token;
	}
	test('the public relay documentation opens outside the window', () => {
		const link = element<HTMLAnchorElement>(page, 'a');
		assert.strictEqual(link.href, 'https://docs.iroh.computer/relays/rate-limiting');
		click(link);
		assert.strictEqual(fixture.openedDocumentation, 1);
	});
	test('the running configuration is described', () => {
		assert.ok(page.querySelector('.connection-active-description')?.textContent.includes('public'));
	});
	test('choosing custom relays commits nothing while the list is empty', async () => {
		await custom();
		assert.strictEqual(fixture.configuration.updateCount(), 0);
	});
	test('an invalid relay address is marked and not committed', async () => {
		const { url } = await custom();
		change(url, 'http://remote.example/');
		await settle();
		assert.strictEqual(fixture.configuration.updateCount(), 0);
		assert.strictEqual(url.getAttribute('aria-invalid'), 'true');
	});
	test('a corrected relay address is committed and no longer marked', async () => {
		const { url } = await custom();
		change(url, 'http://remote.example/');
		await settle();
		change(url, 'https://relay.example/');
		await settle();
		assert.strictEqual(fixture.configuration.updateCount(), 1);
		assert.strictEqual(url.getAttribute('aria-invalid'), 'false');
	});
	test('an authenticated relay without a token is marked and not committed', async () => {
		const { url, auth, token } = await custom();
		change(url, 'https://relay.example/');
		await settle();
		auth.checked = true;
		change(auth);
		await settle();
		assert.strictEqual(fixture.configuration.updateCount(), 1);
		assert.strictEqual(token.getAttribute('aria-invalid'), 'true');
	});
	test('a token is committed with its relay', async () => {
		await authenticatedRelay();
		assert.deepStrictEqual(fixture.configuration.updates.at(-1)?.value, [{ url: 'https://relay.example/', authenticated: true, replacementToken: 'secret-token' }]);
	});
	test('a saved token is cleared from its field and never stored', async () => {
		const token = await authenticatedRelay();
		assert.strictEqual(token.value, '');
		assert.ok(!JSON.stringify(fixture.configuration.getValue<unknown>('connection.relays')).includes('secret-token'));
	});
	test('a saved token is offered for reuse', async () => {
		await authenticatedRelay();
		assert.ok(page.querySelector('.relay-editor')?.textContent.includes('Leave blank to keep the saved token'));
	});
	test('choosing public relays commits an empty list', async () => {
		await authenticatedRelay();
		const publicMode = element<HTMLInputElement>(page, 'input[type="radio"][value="public"]');
		publicMode.checked = true;
		change(publicMode);
		await settle();
		assert.deepStrictEqual(fixture.configuration.getValue<unknown>('connection.relays'), []);
	});
	test('relays are disabled, with the reason, while they cannot be read', () => {
		fixture.configuration.setError('relays', 'Connection settings are unavailable because the connection engine could not start. Restart VniLink to try again.');
		assert.ok(element(page, 'input[type="radio"][value="public"]').matches(':disabled'));
		assert.ok(page.querySelector('.settings-status')?.textContent.includes('unavailable'));
	});
});

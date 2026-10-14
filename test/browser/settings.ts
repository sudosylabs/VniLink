import { Emitter } from '../../src/base/common/event.js';
import { defaultConfiguration, type IConfigurationService, type ConfigurationSnapshot } from '../../src/platform/configuration/common/configuration.js';
import { AppearancePage } from '../../src/workbench/browser/features/settings/browser/appearancePage.js';
import { ConnectionPage } from '../../src/workbench/browser/features/settings/browser/connectionPage.js';
import { ConnectionReadbackError, type ConnectionSettings, type IConnectionService } from '../../src/workbench/services/connection/common/connectionService.js';
function assert(condition: boolean, message: string): void {
	if (!condition) {
		throw new Error(message);
	}
}
async function settle(): Promise<void> {
	await new Promise<void>(resolve => {
		setTimeout(resolve, 0);
	});
}

export async function verifySettingsPages(): Promise<void> {
	let saved: ConnectionSettings = { restartRequired: false, saved: [], active: { publicRelay: true, urls: [] } };
	let failure: 'none' | 'write' | 'readback' = 'none';
	let writes = 0;
	const connection: IConnectionService = {
		_serviceBrand: undefined, available: true,
		load: () => Promise.resolve(saved),
		save: update => {
			writes++;
			if (failure === 'write') {
				return Promise.reject(new Error('Save failed'));
			}
			if (failure === 'readback') {
				return Promise.reject(new ConnectionReadbackError());
			}
			saved = { ...saved, restartRequired: update.mode === 'custom', saved: update.mode === 'public' ? [] : update.relays.map(({ url, authenticated }) => ({ url, authenticated })) };
			return Promise.resolve(saved);
		},
	};
	const applicationChanges = new Emitter<void>();
	const page = new ConnectionPage(connection, { _serviceBrand: undefined, restartRequired: false, onDidChangeRestartRequired: applicationChanges.event, restart: () => Promise.resolve(), openRelayDocumentation: () => Promise.resolve() });
	try {
		await settle();
		assert(page.element.querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled === true, 'Unchanged relay settings allowed a save');
		assert(page.element.querySelector('a')?.href === 'https://docs.iroh.computer/relays/rate-limiting', 'Public relay documentation missing');
		const form = page.element.querySelector('form');
		const mode = page.element.querySelector<HTMLInputElement>('input[type="radio"][value="custom"]');
		assert(!!form && !!mode, 'Connection form missing');
		if (!form || !mode) {
			return;
		}
		mode.checked = true; mode.dispatchEvent(new Event('change'));
		const url = page.element.querySelector<HTMLInputElement>('.relay-editor input[type="text"]');
		const auth = page.element.querySelector<HTMLInputElement>('.relay-editor input[type="checkbox"]');
		const token = page.element.querySelector<HTMLInputElement>('.relay-editor input[type="password"]');
		assert(!!url && !!auth && !!token, 'Custom relay controls missing');
		if (!url || !auth || !token) {
			return;
		}
		url.value = 'http://remote.example/';
		form.dispatchEvent(new Event('submit', { cancelable: true })); await settle();
		assert(writes === 0 && url.getAttribute('aria-invalid') === 'true', 'Invalid URL was submitted');
		const publicMode = page.element.querySelector<HTMLInputElement>('input[type="radio"][value="public"]');
		assert(!!page.element.querySelector('.settings-status')?.textContent, 'Missing custom relay validation summary');
		if (publicMode) {
			publicMode.checked = true; publicMode.dispatchEvent(new Event('change'));
		}
		assert(page.element.querySelector('.settings-status')?.textContent === '', 'Custom relay validation leaked into public mode');
		mode.checked = true; mode.dispatchEvent(new Event('change'));

		url.value = 'https://relay.example/'; auth.checked = true; auth.dispatchEvent(new Event('change'));
		form.dispatchEvent(new Event('submit', { cancelable: true })); await settle();
		assert(writes === 0 && token.getAttribute('aria-invalid') === 'true', 'Missing token was submitted');
		token.value = 'opaque-token'; failure = 'write';
		form.dispatchEvent(new Event('submit', { cancelable: true })); await settle();
		assert(token.value === 'opaque-token' && url.value === 'https://relay.example/', 'Failed save discarded the draft');
		assert(page.element.querySelector('.settings-status')?.textContent.includes('unchanged') === true, 'Failed write was reported as saved');
		failure = 'none';
		form.dispatchEvent(new Event('submit', { cancelable: true })); await settle();
		assert(token.value === '', 'Old credential input was not cleared');
		assert(page.element.querySelector<HTMLInputElement>('input[type="password"]')?.value === '', 'Saved token was rendered');
		assert(page.element.querySelector<HTMLElement>('.connection-current > .connection-description')?.hidden === false, 'Restart requirement missing');
		assert(page.element.querySelector('.connection-active-description')?.textContent.toLowerCase().includes('public') === true, 'Save was presented as an active relay change');
		failure = 'readback';
		const replacement = page.element.querySelector<HTMLInputElement>('input[type="password"]'); if (replacement) {
			replacement.value = 'replacement-token';
		}
		form.dispatchEvent(new Event('submit', { cancelable: true })); await settle();
		assert(page.element.querySelector('.settings-fields')?.hasAttribute('disabled') === true, 'Read-back failure allowed edits against stale settings');
		assert(page.element.querySelector('.settings-status')?.textContent.startsWith('Saved,') === true, 'Committed save was reported as unchanged');
	} finally {
		page.dispose(); applicationChanges.dispose();
	}

	const changed = new Emitter<ConfigurationSnapshot>();
	let snapshot: ConfigurationSnapshot = { revision: 0, values: defaultConfiguration, error: null };
	let appearanceWriteFails = false;
	const configuration: IConfigurationService = {
		_serviceBrand: undefined, get snapshot() {
			return snapshot;
		},
		onDidChangeConfiguration: changed.event, initialize: () => Promise.resolve(),
		updateValue: change => {
			if (appearanceWriteFails) {
				return Promise.reject(new Error('Disk failure'));
			}
			snapshot = { revision: snapshot.revision + 1, values: { ...snapshot.values, [change.key]: change.value }, error: null };
			changed.fire(snapshot); return Promise.resolve();
		},
	};
	const appearance = new AppearancePage(configuration);
	try {
		assert(appearance.element.querySelector<HTMLElement>('.appearance-status')?.hidden === true, 'Appearance showed a routine save notice');
		const dark = appearance.element.querySelector<HTMLInputElement>('input[type="radio"][value="dark"]');
		const light = appearance.element.querySelector<HTMLInputElement>('input[type="radio"][value="light"]');
		const material = appearance.element.querySelector<HTMLInputElement>('input[type="checkbox"]');
		assert(!!dark && !!light && !!material, 'Appearance controls missing');
		if (!dark || !light || !material) {
			return;
		}
		dark.checked = true; dark.dispatchEvent(new Event('change')); await settle();
		assert(snapshot.values['appearance.theme'] === 'dark', 'Theme change bypassed configuration');
		assert(appearance.element.querySelector<HTMLElement>('.appearance-status')?.hidden === true, 'Successful save left unnecessary feedback');
		material.checked = false; material.dispatchEvent(new Event('change')); await settle();
		assert(!snapshot.values['appearance.nativeBackground'], 'Material preference bypassed configuration');
		appearanceWriteFails = true;
		light.checked = true; light.dispatchEvent(new Event('change')); await settle();
		assert(appearance.element.querySelector<HTMLInputElement>('input[type="radio"]:checked')?.value === 'light' && snapshot.values['appearance.theme'] === 'dark', 'Appearance save failure discarded draft or changed saved value');
		assert(appearance.element.querySelector<HTMLElement>('.appearance-status')?.hidden === false, 'Save failure was hidden');
		appearanceWriteFails = false;
		appearance.element.querySelector<HTMLButtonElement>('.settings-button')?.click(); await settle();
		assert(snapshot.values['appearance.theme'] === 'light', 'Appearance retry did not save the draft');
		appearance.dispose(); assert(!changed.hasListeners(), 'Appearance kept its configuration listener');
	} finally {
		appearance.dispose(); changed.dispose();
	}
	const blockedChanges = new Emitter<ConfigurationSnapshot>();
	const blocked = new AppearancePage({ ...configuration, snapshot: { revision: 0, values: defaultConfiguration, error: 'Invalid settings file' }, onDidChangeConfiguration: blockedChanges.event });
	try {
		assert(blocked.element.querySelector<HTMLInputElement>('input[type="radio"]')?.disabled === true, 'Corrupt configuration remained writable');
	} finally {
		blocked.dispose();
		blockedChanges.dispose();
	}
}

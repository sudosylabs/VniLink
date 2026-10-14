import { Emitter } from '../../src/base/common/event.js';
import { FilesPage } from '../../src/workbench/contrib/settings/browser/filesPage.js';
import { TerminalPage } from '../../src/workbench/contrib/settings/browser/terminalPage.js';
import { defaultConfiguration, type IConfigurationService, type ConfigurationSnapshot } from '../../src/platform/configuration/common/configuration.js';
import type { ITerminalSettingsService } from '../../src/workbench/services/terminal/common/terminalSettingsService.js';

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

export async function verifyFileTerminalSettings(): Promise<void> {
	const changed = new Emitter<ConfigurationSnapshot>();
	let snapshot: ConfigurationSnapshot = { revision: 0, values: defaultConfiguration, error: null };
	let writes = 0;
	const configuration: IConfigurationService = {
		_serviceBrand: undefined, get snapshot() {
			return snapshot;
		}, onDidChangeConfiguration: changed.event, initialize: () => Promise.resolve(), updateValue: change => {
			writes++;
			snapshot = { ...snapshot, revision: snapshot.revision + 1, values: { ...snapshot.values, [change.key]: change.value } };
			changed.fire(snapshot);
			return Promise.resolve();
		}
	};
	const files = new FilesPage(configuration);
	try {
		const form = files.element.querySelector('form');
		const save = files.element.querySelector<HTMLButtonElement>('button[type="submit"]');
		assert(save?.disabled === true, 'Unchanged shared folders allowed a save');
		files.element.querySelector<HTMLButtonElement>('fieldset > button')?.click();
		form?.dispatchEvent(new Event('submit', { cancelable: true }));
		await settle();
		assert(writes === 0, 'An empty folder was saved');
		const input = files.element.querySelector<HTMLInputElement>('input');
		if (input) {
			input.value = '/test/shared';
			input.dispatchEvent(new Event('input', { bubbles: true }));
		}
		form?.dispatchEvent(new Event('submit', { cancelable: true }));
		await settle();
		assert(writes === 1 && snapshot.values['files.roots'][0] === '/test/shared', 'Shared folder draft was not persisted');
	} finally {
		files.dispose();
		changed.dispose();
	}
	let fail = false;
	let saves = 0;
	const policy = { allow_persistence: true, max_sessions: 16, max_sessions_per_peer: 16, history_bytes: 1048576, replay_bytes: 8192, detached_timeout_secs: 300 };
	const service: ITerminalSettingsService = {
		_serviceBrand: undefined, available: true, load: () => fail ? Promise.reject(new Error('Failed')) : Promise.resolve(policy), save: draft => {
			saves++;
			return Promise.resolve(draft);
		}
	};
	const terminal = new TerminalPage(service);
	try {
		await settle();
		const form = terminal.element.querySelector('form');
		const total = terminal.element.querySelector<HTMLInputElement>('[data-limit="max_sessions"]');
		if (total) {
			total.value = '0';
			total.dispatchEvent(new Event('input', { bubbles: true }));
		}
		form?.dispatchEvent(new Event('submit', { cancelable: true }));
		await settle();
		assert(saves === 0, 'Invalid terminal limits were saved');
		fail = true;
		const refresh = terminal.element.querySelector<HTMLButtonElement>('.settings-text-button');
		refresh?.click();
		await settle();
		assert(terminal.element.querySelector<HTMLFieldSetElement>('fieldset')?.disabled === true && refresh?.disabled === false && !refresh.closest('fieldset'), 'Failed terminal load prevented refresh recovery');
	} finally {
		terminal.dispose();
	}
}

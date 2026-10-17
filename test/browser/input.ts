import { InputPage } from '../../src/workbench/contrib/settings/inputPage.js';
import { defaultConfiguration, type IConfigurationSnapshotService, type ConfigurationSnapshot } from '../../src/platform/configuration/common/configurationSnapshot.js';
import { Emitter } from '../../src/base/common/event.js';

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

export async function verifyInputPage(): Promise<void> {
	const changes = new Emitter<ConfigurationSnapshot>();
	let snapshot = { revision: 0, values: defaultConfiguration, error: null };
	let writes = 0;
	let fail = false;
	const service: IConfigurationSnapshotService = {
		_serviceBrand: undefined, get snapshot() {
			return snapshot;
		}, onDidChangeConfiguration: changes.event, initialize: () => Promise.resolve(), updateValue: change => {
			writes++;
			if (fail) {
				return Promise.reject(new Error('Refused'));
			}
			snapshot = { ...snapshot, revision: snapshot.revision + 1, values: { ...snapshot.values, [change.key]: change.value } };
			changes.fire(snapshot);
			return Promise.resolve();
		}
	};
	const page = new InputPage(service);
	document.body.append(page.element);
	try {
		const fields = page.element.querySelector<HTMLFieldSetElement>('fieldset');
		const save = page.element.querySelector<HTMLButtonElement>('button[type="submit"]');
		assert(fields?.disabled === false && save?.disabled === true, 'Offline defaults were not editable');
		assert(!page.element.querySelector('select:not(fieldset select)'), 'Defaults still require a device selector');
		const mode = page.element.querySelector<HTMLSelectElement>('fieldset select');
		if (mode) {
			mode.value = 'Map';
			mode.dispatchEvent(new Event('change', { bubbles: true }));
		}
		assert(save?.disabled === false, 'Changed defaults cannot be saved');
		save?.click();
		await settle();
		assert(writes === 1 && snapshot.values['input.defaults'].mode === 'Map' && save?.disabled === true, 'Accepted defaults did not establish the baseline');
		if (mode) {
			mode.value = 'Translate';
			mode.dispatchEvent(new Event('change', { bubbles: true }));
		}
		fail = true;
		save?.click();
		await settle();
		assert(mode?.value === 'Translate' && snapshot.values['input.defaults'].mode === 'Map' && save?.disabled === false, 'Failed defaults save lost the draft');
	} finally {
		page.dispose();
		changes.dispose();
	}
}

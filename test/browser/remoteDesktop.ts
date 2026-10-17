import { DesktopCodecSection } from '../../src/workbench/contrib/settings/desktopCodecSection.js';
import { DesktopDefaultsSection } from '../../src/workbench/contrib/settings/desktopDefaultsSection.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { IConfigurationSnapshotService, defaultConfiguration, type ConfigurationSnapshot } from '../../src/platform/configuration/common/configurationSnapshot.js';
import { Event as LifecycleEvent, Emitter } from '../../src/base/common/event.js';
import { RemoteDesktopPage } from '../../src/workbench/contrib/settings/remoteDesktopPage.js';
import { DesktopQualityEditor } from '../../src/workbench/contrib/settings/desktopQualityEditor.js';
import { IRemoteDesktopService, type DesktopSettings } from '../../src/workbench/services/remoteDesktop/remoteDesktopService.js';

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

export async function verifyRemoteDesktop(): Promise<void> {
	const changed = new Emitter<ConfigurationSnapshot>();
	let snapshot: ConfigurationSnapshot = { revision: 0, values: defaultConfiguration, error: null };
	let defaultsWrites = 0;
	let defaultsFailure = false;
	const defaults = new DesktopDefaultsSection({
		_serviceBrand: undefined, get snapshot() {
			return snapshot;
		}, onDidChangeConfiguration: changed.event, initialize: () => Promise.resolve(), updateValue: change => {
			defaultsWrites++;
			if (defaultsFailure) {
				return Promise.reject(new Error('Disk unavailable'));
			}
			snapshot = { ...snapshot, revision: snapshot.revision + 1, values: { ...snapshot.values, [change.key]: change.value } };
			changed.fire(snapshot);
			return Promise.resolve();
		}
	});
	try {
		const save = defaults.element.querySelector<HTMLButtonElement>('button[type="submit"]');
		assert(save?.disabled === true, 'Unchanged desktop defaults allowed saving');
		defaults.element.querySelector<HTMLButtonElement>('[data-preset="balanced"]')?.click();
		assert(save?.disabled === false, 'Desktop defaults required a live session');
		defaults.element.querySelector('form')?.dispatchEvent(new Event('submit', { cancelable: true }));
		await settle();
		assert(defaultsWrites === 1 && snapshot.values['desktop.quality'].max_bitrate_kbps === 5000 && save?.disabled === true, 'Desktop defaults did not save and clear dirty state');
		defaultsFailure = true;
		defaults.element.querySelector<HTMLButtonElement>('[data-preset="smooth"]')?.click();
		defaults.element.querySelector('form')?.dispatchEvent(new Event('submit', { cancelable: true }));
		await settle();
		assert(defaults.element.querySelector<HTMLInputElement>('[data-quality="max_frame_rate"]')?.value === '60' && save?.disabled === false, 'Failed default save discarded the draft');
	} finally {
		defaults.dispose();
		changed.dispose();
	}
	const state: DesktopSettings = {
		sessions: [], codecs: {
			encode_baseline: false, encode_high: false, decode_baseline: true, decode_high: false, max_width: 3840, max_height: 2160, hardware: true, device: 'test', chroma444: false, additional: [
				{ codec: 'Vp9', encode: true, decode: true, hardware: false },
				{ codec: 'Av1', encode: false, decode: true, hardware: false },
				{ codec: 'H265', encode: true, decode: false, hardware: true },
			]
		}
	};
	const service: IRemoteDesktopService = { _serviceBrand: undefined, available: true, load: () => Promise.resolve(state), displays: () => Promise.resolve([]), apply: () => Promise.reject(new Error('Session controls do not belong here')) };
	const codecChanged = new Emitter<ConfigurationSnapshot>();
	let codecSnapshot: ConfigurationSnapshot = { revision: 0, values: defaultConfiguration, error: null };
	let codecFailure = false;
	const codecSection = new DesktopCodecSection({
		_serviceBrand: undefined, get snapshot() {
			return codecSnapshot;
		}, onDidChangeConfiguration: codecChanged.event, initialize: () => Promise.resolve(), updateValue: change => {
			if (codecFailure) {
				return Promise.reject(new Error('Disk refused'));
			}
			codecSnapshot = { ...codecSnapshot, revision: codecSnapshot.revision + 1, values: { ...codecSnapshot.values, [change.key]: change.value } };
			codecChanged.fire(codecSnapshot);
			return Promise.resolve();
		}
	}, service);
	try {
		await settle();
		const select = codecSection.element.querySelector<HTMLSelectElement>('select');
		const save = codecSection.element.querySelector<HTMLButtonElement>('button');
		if (!select || !save) {
			throw new Error('Codec controls missing');
		}
		assert(select.value === 'auto' && select.options.length === 4 && save.disabled, 'Codec default/options incorrect');
		select.value = 'vp9';
		select.dispatchEvent(new Event('change'));
		assert(!save.disabled, 'Codec choice did not become dirty');
		codecSection.element.querySelector('form')?.dispatchEvent(new Event('submit', { cancelable: true }));
		await settle();
		assert(codecSnapshot.values['desktop.codec'] === 'vp9' && save.disabled, 'Codec choice did not persist');
		codecFailure = true;
		select.value = 'av1';
		select.dispatchEvent(new Event('change'));
		codecSection.element.querySelector('form')?.dispatchEvent(new Event('submit', { cancelable: true }));
		await settle();
		assert(select.value === 'av1' && !save.disabled && codecSection.element.textContent.includes('Disk refused'), 'Codec error discarded draft');
	} finally {
		codecSection.dispose();
		codecChanged.dispose();
	}
	const instantiation = new InstantiationService(new ServiceCollection(
		[IConfigurationSnapshotService, { _serviceBrand: undefined, snapshot: { revision: 0, values: defaultConfiguration, error: null }, onDidChangeConfiguration: LifecycleEvent.None, initialize: () => Promise.resolve(), updateValue: () => Promise.resolve() }],
		[IRemoteDesktopService, service],
	), true);
	const page = new RemoteDesktopPage(instantiation);
	try {
		await settle();
		assert(page.element.textContent.includes('Default stream quality'), 'Global defaults missing without a session');
		assert(!page.element.textContent.includes('Session overrides') && !page.element.textContent.includes('Video processing on this computer') && !page.element.textContent.includes('Displays'), 'Session-only sections remain in global settings');
		const options = Array.from(page.element.querySelectorAll<HTMLOptionElement>('select option')).map(option => option.value);
		assert(!options.includes('h265') && !options.includes('vp8') && options.includes('auto') && options.includes('vp9'), 'Unsupported viewer codecs were offered');
	} finally {
		page.dispose();
		instantiation.dispose();
	}
	const editor = new DesktopQualityEditor();
	try {
		editor.setValue({ max_bitrate_kbps: 1, max_frame_rate: 1, max_width: 1, max_height: 1 });
		const input = editor.element.querySelector<HTMLInputElement>('[data-quality="max_frame_rate"]');
		if (input) {
			input.value = '61';
		}
		assert(editor.value() === undefined, 'Invalid limits accepted');
	} finally {
		editor.dispose();
	}
}

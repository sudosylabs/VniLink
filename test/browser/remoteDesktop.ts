import { DesktopCodecSection } from '../../src/workbench/browser/features/settings/browser/desktopCodecSection.js';
import { DesktopDefaultsSection } from '../../src/workbench/browser/features/settings/browser/desktopDefaultsSection.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { IConfigurationService, defaultConfiguration, type ConfigurationSnapshot } from '../../src/platform/configuration/common/configuration.js';
import { Event as LifecycleEvent, Emitter } from '../../src/base/common/event.js';
import { RemoteDesktopPage } from '../../src/workbench/browser/features/settings/browser/remoteDesktopPage.js';
import { DesktopQualityEditor } from '../../src/workbench/browser/features/settings/browser/desktopQualityEditor.js';
import type { IRemoteDesktopService, DesktopSettings, DesktopTarget } from '../../src/workbench/services/remoteDesktop/common/remoteDesktopService.js';
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
	const changed = new Emitter<ConfigurationSnapshot>(); let snapshot: ConfigurationSnapshot = { revision: 0, values: defaultConfiguration, error: null }; let defaultsWrites = 0; let defaultsFailure = false;
	const defaults = new DesktopDefaultsSection({
		_serviceBrand: undefined, get snapshot() {
			return snapshot;
		}, onDidChangeConfiguration: changed.event, initialize: () => Promise.resolve(), updateValue: change => {
			defaultsWrites++; if (defaultsFailure) {
				return Promise.reject(new Error('Disk unavailable'));
			}
			snapshot = { ...snapshot, revision: snapshot.revision + 1, values: { ...snapshot.values, [change.key]: change.value } }; changed.fire(snapshot); return Promise.resolve();
		}
	});
	try {
		const save = defaults.element.querySelector<HTMLButtonElement>('button[type="submit"]'); assert(save?.disabled === true, 'Unchanged desktop defaults allowed saving');
		defaults.element.querySelector<HTMLButtonElement>('[data-preset="balanced"]')?.click(); assert(save?.disabled === false, 'Desktop defaults required a live session'); defaults.element.querySelector('form')?.dispatchEvent(new Event('submit', { cancelable: true })); await settle(); assert(defaultsWrites === 1 && snapshot.values['desktop.quality'].max_bitrate_kbps === 5000 && save?.disabled === true, 'Desktop defaults did not save and clear dirty state');
		defaultsFailure = true; defaults.element.querySelector<HTMLButtonElement>('[data-preset="smooth"]')?.click(); defaults.element.querySelector('form')?.dispatchEvent(new Event('submit', { cancelable: true })); await settle(); assert(defaults.element.querySelector<HTMLInputElement>('[data-quality="max_frame_rate"]')?.value === '60' && save?.disabled === false, 'Failed default save discarded the draft');
	} finally {
		defaults.dispose(); changed.dispose();
	}


	const codecChanged = new Emitter<ConfigurationSnapshot>(); let codecSnapshot: ConfigurationSnapshot = { revision: 0, values: defaultConfiguration, error: null }; let codecFailure = false;
	const codecSection = new DesktopCodecSection({
		_serviceBrand: undefined, get snapshot() {
			return codecSnapshot;
		}, onDidChangeConfiguration: codecChanged.event, initialize: () => Promise.resolve(), updateValue: change => {
			if (codecFailure) {
				return Promise.reject(new Error('Disk refused'));
			}
			codecSnapshot = { ...codecSnapshot, revision: codecSnapshot.revision + 1, values: { ...codecSnapshot.values, [change.key]: change.value } }; codecChanged.fire(codecSnapshot); return Promise.resolve();
		}
	});
	try {
		const select = codecSection.element.querySelector<HTMLSelectElement>('select'); const save = codecSection.element.querySelector<HTMLButtonElement>('button');
		if (!select || !save) {
			throw new Error('Codec controls missing');
		}
		assert(select.value === 'auto' && select.options.length === 6 && save.disabled, 'Codec default/options incorrect');
		select.value = 'vp9'; select.dispatchEvent(new Event('change')); assert(!save.disabled, 'Codec choice did not become dirty');
		codecSection.element.querySelector('form')?.dispatchEvent(new Event('submit', { cancelable: true })); await settle(); assert(codecSnapshot.values['desktop.codec'] === 'vp9' && save.disabled, 'Codec choice did not persist');
		codecFailure = true; select.value = 'av1'; select.dispatchEvent(new Event('change')); codecSection.element.querySelector('form')?.dispatchEvent(new Event('submit', { cancelable: true })); await settle(); assert(select.value === 'av1' && !save.disabled && codecSection.element.textContent.includes('Disk refused'), 'Codec error discarded draft');
	} finally {
		codecSection.dispose(); codecChanged.dispose();
	}
	const codecs = { encode_baseline: false, encode_high: false, decode_baseline: true, decode_high: false, max_width: 3840, max_height: 2160, hardware: true, device: 'test', chroma444: false };
	let state: DesktopSettings = { sessions: [{ id: '1', generation: '7', role: 'Controller', name: '<Work>', quality: { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 } }], codecs };
	let writes = 0; let target: DesktopTarget | undefined; let fail = false;
	const service: IRemoteDesktopService = {
		_serviceBrand: undefined, available: true, load: () => Promise.resolve(state), displays: () => Promise.resolve([{ id: '9', origin_x: 0, origin_y: 0, width: 1920, height: 1080, scale_percent: 100, rotation_deg: 0, revision: '1' }]), apply: (owner, quality) => {
			writes++; target = owner; if (fail) {
				return Promise.reject(new Error('Session ended'));
			}
			state = { ...state, sessions: [{ ...state.sessions[0], quality }] }; return Promise.resolve(state);
		}
	};
	const instantiation = new InstantiationService(new ServiceCollection([IConfigurationService, { _serviceBrand: undefined, snapshot: { revision: 0, values: defaultConfiguration, error: null }, onDidChangeConfiguration: LifecycleEvent.None, initialize: () => Promise.resolve(), updateValue: () => Promise.resolve() }]), true);
	const page = new RemoteDesktopPage(service, instantiation);
	try {
		await settle(); assert(page.element.textContent.includes('1920 × 1080 px'), 'Session display data missing');
		const preset = page.element.querySelector<HTMLButtonElement>('.desktop-content > form [data-preset="smooth"]'); preset?.click(); assert(writes === 0, 'Preset wrote before Apply');
		const form = page.element.querySelector('.desktop-content > form'); form?.dispatchEvent(new Event('submit', { cancelable: true })); await settle(); assert(writes === 1 && target?.generation === '7' && state.sessions[0].quality.max_frame_rate === 60, 'Apply did not target selected session');
		fail = true; form?.dispatchEvent(new Event('submit', { cancelable: true })); await settle(); assert(page.element.querySelector<HTMLFieldSetElement>('.desktop-content > form fieldset')?.disabled === true, 'Failed write left session editable');
		page.dispose(); const before = writes; form?.dispatchEvent(new Event('submit')); await settle(); assert(writes === before, 'Disposed page still writes');
	} finally {
		page.dispose();
	}
	state = { ...state, sessions: [] }; const empty = new RemoteDesktopPage(service, instantiation);
	try {
		await settle(); assert(empty.element.textContent.includes('No active desktop session'), 'Empty desktop page fabricated a session'); assert(empty.element.querySelector<HTMLFieldSetElement>('.desktop-content > form fieldset')?.disabled === true, 'Quality controls enabled without session');
	} finally {
		empty.dispose();
	}
	const editor = new DesktopQualityEditor();
	try {
		editor.setValue({ max_bitrate_kbps: 1, max_frame_rate: 1, max_width: 1, max_height: 1 }); const input = editor.element.querySelector<HTMLInputElement>('[data-quality="max_frame_rate"]'); if (input) {
			input.value = '61';
		} assert(editor.value() === undefined, 'Invalid limits accepted');
	} finally {
		editor.dispose();
	}
}

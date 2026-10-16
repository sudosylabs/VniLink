import { Event as LifecycleEvent, Emitter } from '../../src/base/common/event.js';
import { defaultConfiguration, type ConfigurationSnapshot, type IConfigurationService } from '../../src/platform/configuration/common/configuration.js';
import type { INativeRuntimeService } from '../../src/workbench/services/nativeRuntime/common/nativeRuntimeService.js';
import type { IPermissionService } from '../../src/workbench/services/permissions/common/permissionService.js';
import { RecordingPage } from '../../src/workbench/contrib/settings/browser/recordingPage.js';
import { PowerPage } from '../../src/workbench/contrib/settings/browser/powerPage.js';
import { MediaPage } from '../../src/workbench/contrib/settings/browser/mediaPage.js';
import type { NativeDevice, PowerPolicy } from '../../src/platform/engine/common/engine.js';

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

export async function verifyMediaSettings(): Promise<void> {
	const changed = new Emitter<ConfigurationSnapshot>();
	let snapshot: ConfigurationSnapshot = { revision: 0, values: defaultConfiguration, error: null };
	let failed = false;
	const configuration: IConfigurationService = {
		_serviceBrand: undefined, get snapshot() {
			return snapshot;
		}, onDidChangeConfiguration: changed.event, initialize: () => Promise.resolve(), updateValue: update => {
			if (failed) {
				return Promise.reject(new Error('Disk unavailable'));
			}
			snapshot = { ...snapshot, revision: snapshot.revision + 1, values: { ...snapshot.values, [update.key]: update.value } };
			changed.fire(snapshot);
			return Promise.resolve();
		}
	};
	const permissions: IPermissionService = { _serviceBrand: undefined, onDidFocusWindow: LifecycleEvent.None as LifecycleEvent<void>, execute: () => Promise.resolve({ development: false, permissions: [] }) };
	let policy: PowerPolicy = { incoming_display: false, incoming_system: true, outgoing_display: false, outgoing_system: true, incoming_idle_secs: 300, outgoing_idle_secs: 300, lock_after_control: false };
	let powerWrites = 0;
	let pendingPowerRead: Promise<PowerPolicy> | undefined;
	const devices: NativeDevice[] = [{ id: 'camera-one', name: 'Camera <one>', kind: 'camera' }, { id: 'mic-one', name: 'Microphone', kind: 'microphone' }];
	const unused = (): Promise<never> => Promise.reject(new Error('Not exercised'));
	const runtime: INativeRuntimeService = {
		_serviceBrand: undefined, available: true, recordingDirectory: () => Promise.resolve('/Pictures/VniLink'), load: unused, captureDisplays: unused, startSharing: unused, stopSharing: unused, sharingStatus: unused, startRecording: unused, recordingStatus: unused, stopRecording: unused, releaseRecording: unused, remoteDevices: unused, approveCameras: unused, approveAudio: unused,
		devices: kind => Promise.resolve(devices.filter(device => kind === 'camera' ? device.kind === 'camera' : device.kind === 'microphone')), powerPolicy: () => pendingPowerRead ?? Promise.resolve(policy), powerStatus: () => Promise.resolve([]), savePowerPolicy: next => {
			powerWrites++;
			policy = next;
			return Promise.resolve(policy);
		},
	};
	const recording = new RecordingPage(configuration, runtime);
	const power = new PowerPage(runtime, permissions);
	const media = new MediaPage(configuration, runtime, permissions);
	document.body.append(recording.element, power.element, media.element);
	try {
		await settle();
		const submit = (element: HTMLElement): void => {
			element.querySelector('form')?.dispatchEvent(new Event('submit', { cancelable: true }));
		};
		const size = recording.element.querySelector<HTMLInputElement>('[data-setting="maxBytes"]');
		assert(size?.value === '1024', 'Recording defaults failed to load');
		if (size) {
			size.value = '2048';
			size.dispatchEvent(new Event('input', { bubbles: true }));
		}
		submit(recording.element);
		await settle();
		assert(snapshot.values['recording.defaults'].maxBytes === '2147483648', 'Recording defaults did not persist in bytes');
		failed = true;
		if (size) {
			size.value = '4096';
			size.dispatchEvent(new Event('input', { bubbles: true }));
		}
		submit(recording.element);
		await settle();
		assert(size?.value === '4096' && recording.element.textContent.includes('Disk unavailable'), 'Failed save discarded recording draft');
		failed = false;
		const lock = power.element.querySelector<HTMLInputElement>('[data-setting="lock_after_control"]');
		assert(!!lock, 'Power controls missing');
		let finishRead: ((value: PowerPolicy) => void) | undefined;
		pendingPowerRead = new Promise<PowerPolicy>(resolve => {
			finishRead = resolve;
		});
		const refresh = power.refresh();
		if (lock) {
			lock.checked = true;
			lock.dispatchEvent(new Event('change', { bubbles: true }));
			assert(!lock.disabled, 'Editing during a background read disabled the settings form');
		}
		finishRead?.(policy);
		await refresh;
		pendingPowerRead = undefined;
		assert(lock?.checked === true, 'Background refresh discarded an edit made while reading settings');
		pendingPowerRead = Promise.reject(new Error('Temporary read failure'));
		await power.refresh();
		assert(power.element.querySelector<HTMLFieldSetElement>('fieldset')?.disabled === true, 'Failed read left uncertain settings writable');
		pendingPowerRead = undefined;
		await power.refresh();
		assert(lock?.checked === true, 'Read recovery discarded an unsaved power draft');
		assert(power.element.querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled === false, 'Recovered dirty draft could not be saved');
		submit(power.element);
		await settle();
		assert(powerWrites === 1 && policy.lock_after_control, 'Power default did not reach the runtime service');
		const camera = media.element.querySelector<HTMLSelectElement>('select[aria-label="Cameras"]');
		assert(media.element.querySelectorAll('.settings-select > .vnilink-icon').length === 3, 'Media sources did not use the shared select presentation');
		assert(!!camera && media.element.textContent.includes('Camera <one>') && !media.element.querySelector('one'), 'Media enumeration was missing or treated names as HTML');
		if (camera) {
			camera.value = 'camera-one';
			camera.dispatchEvent(new Event('change', { bubbles: true }));
		}
		submit(media.element);
		await settle();
		assert(snapshot.values['media.sources'].cameras.includes('camera-one'), 'Source selection did not reach configuration');
		const buttons = [power.element, media.element].map(element => element.querySelector<HTMLButtonElement>('button[type="submit"]'));
		assert(buttons.every(button => button?.disabled === true), 'Saved defaults remained dirty');
		power.dispose();
		if (lock) {
			lock.checked = false;
			lock.dispatchEvent(new Event('change', { bubbles: true }));
		}
		submit(power.element);
		await settle();
		assert(powerWrites === 1, 'Disposed settings retained write listeners');
	} finally {
		recording.dispose();
		power.dispose();
		media.dispose();
		changed.dispose();
	}
}

import { Emitter } from '../../src/base/common/event.js';
import { Disposable } from '../../src/base/common/lifecycle.js';
import type { ControlRequestInfo } from '../../src/platform/engine/common/engine.js';
import type { DeviceSummary, IDeviceService, PairingFile } from '../../src/workbench/services/devices/common/deviceService.js';
import { DevicesPage } from '../../src/workbench/contrib/home/browser/devicesPage.js';

function assert(value: unknown, message: string): asserts value {
	if (!value) {
		throw new Error(message);
	}
}

async function settle(): Promise<void> {
	await new Promise<void>(resolve => {
		setTimeout(resolve, 0);
	});
}

export type DeviceFixtureState = 'empty' | 'populated' | 'selected' | 'unavailable';

/** Explicit test data; the production page always receives the real service. */
export class DevicePageFixture extends Disposable implements IDeviceService {
	declare public readonly _serviceBrand: undefined;
	public readonly available = true;
	public state: 'loading' | 'ready' | 'unavailable' = 'ready';
	private readonly changed = this._register(new Emitter<readonly DeviceSummary[]>());
	public readonly onDidChangeDevices = this.changed.event;
	private readonly stateChanged = this._register(new Emitter<void>());
	public readonly onDidChangeState = this.stateChanged.event;
	public devices: readonly DeviceSummary[] = [];
	public readonly page: DevicesPage;
	private readonly host = document.createElement('div');
	public connects: string[] = [];
	public confirmation = '';
	public renameResult: Promise<void> | undefined;
	public incoming: ControlRequestInfo | null = null;

	constructor(state: DeviceFixtureState = 'populated') {
		super();
		if (state === 'populated' || state === 'selected') {
			this.devices = [
				{ id: 'a'.repeat(64), name: 'Studio Mac', os: 'macos', presence: 'connected' },
				{ id: 'b'.repeat(64), name: 'Sudosy', os: 'linux', presence: 'disconnected' },
				{ id: 'c'.repeat(64), name: 'Office PC', os: 'windows', presence: 'reconnecting' },
			];
		} else if (state === 'unavailable') {
			this.state = 'unavailable';
		}
		this.page = this._register(new DevicesPage(this));
		this.host.className = 'home-content device-fixture';
		this.host.style.cssText = 'position:fixed;top:var(--titlebar-height);left:190px;right:0;bottom:0;z-index:10;';
		this.host.append(this.page.element);
		document.body.append(this.host);
		if (state === 'selected') {
			this.page.element.querySelector<HTMLButtonElement>('.device-row')?.click();
		}
	}

	public initialize(): Promise<void> {
		return Promise.resolve();
	}

	public refresh(): Promise<void> {
		return Promise.resolve();
	}

	public connect(id: string): Promise<void> {
		this.connects.push(id);
		return Promise.resolve();
	}

	public async rename(id: string, name: string): Promise<void> {
		await this.renameResult;
		this.update(this.devices.map(device => device.id === id ? { ...device, name } : device));
	}

	public forget(id: string): Promise<void> {
		this.update(this.devices.filter(device => device.id !== id));
		return Promise.resolve();
	}

	public exportPairingFile(): Promise<PairingFile> {
		return Promise.resolve({ bytes: [86, 78, 76, 70], check: '042', expiresAt: Date.now() + 900000 });
	}

	public pair(_bytes: Uint8Array, digits: string): Promise<string> {
		this.confirmation = digits;
		return Promise.resolve('b'.repeat(64));
	}

	public pairingCheck(): Promise<string> {
		return Promise.resolve('731');
	}

	public incomingPairing(): Promise<ControlRequestInfo | null> {
		return Promise.resolve(this.incoming);
	}

	public approvePairing(digits: string): Promise<void> {
		this.confirmation = digits;
		this.incoming = null;
		return Promise.resolve();
	}

	public rejectPairing(): Promise<void> {
		this.incoming = null;
		return Promise.resolve();
	}

	public update(devices: readonly DeviceSummary[]): void {
		this.devices = devices;
		this.changed.fire(devices);
	}

	public override dispose(): void {
		super.dispose();
		this.host.remove();
	}
}

export async function verifyDevices(): Promise<void> {
	const fixture = new DevicePageFixture();
	const page = fixture.page.element;
	try {
		const tabCount = document.querySelectorAll('[role="tab"]').length;
		const rows = [...page.querySelectorAll<HTMLButtonElement>('.device-row')];
		assert(rows.length === 3, 'Device fixture did not render');
		assert(fixture.page.actions.length === 0 && !page.querySelector('.devices-toolbar'), 'Removed device actions are still presented');
		rows[1].focus();
		rows[1].click();
		assert(!page.querySelector<HTMLElement>('.device-panel')?.hidden, 'Selecting a device did not open details');
		assert(document.querySelectorAll('[role="tab"]').length === tabCount, 'Device selection opened a titlebar tab');
		fixture.update(fixture.devices.map(device => device.id === rows[1].dataset.deviceId ? { ...device, presence: 'connecting' } : device));
		assert(document.activeElement === rows[1] && page.querySelectorAll('.device-row')[1] === rows[1], 'Presence update replaced the focused row');
		assert(rows[1].getAttribute('aria-pressed') === 'true', 'Presence update lost selection');
		rows[1].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
		assert(document.activeElement === rows[2], 'Arrow navigation failed');
		page.querySelector<HTMLButtonElement>('.device-close')?.click();
		assert(document.activeElement === rows[1] && page.querySelector<HTMLElement>('.device-panel')?.hidden, 'Close did not restore list focus');
		fixture.update(fixture.devices.map(device => ({ ...device, presence: 'disconnected' })));
		rows[1].click();
		page.querySelector<HTMLButtonElement>('.device-panel .primary')?.click();
		await settle();
		assert(fixture.connects[0] === 'b'.repeat(64), 'Connect used the wrong endpoint');
		const name = page.querySelector<HTMLInputElement>('.device-rename input');
		const form = page.querySelector<HTMLFormElement>('.device-rename');
		assert(name && form, 'Rename controls missing');
		name.value = '<b>Personal</b>';
		name.dispatchEvent(new Event('input'));
		form.dispatchEvent(new Event('submit', { cancelable: true }));
		await settle();
		assert(rows[1].textContent.includes('<b>Personal</b>') && !page.querySelector('b'), 'Rename rendered untrusted markup');
		let reject: (reason: Error) => void = () => {
			throw new Error('No pending request');
		};
		fixture.renameResult = new Promise<void>((_resolve, fail) => {
			reject = fail;
		});
		name.value = 'Pending';
		name.dispatchEvent(new Event('input'));
		form.dispatchEvent(new Event('submit', { cancelable: true }));
		rows[0].click();
		reject(new Error('Old device failure'));
		await settle();
		assert(!page.querySelector('.device-panel')?.textContent.includes('Old device failure'), 'Late command error crossed selections');
		page.querySelector<HTMLButtonElement>('.device-forget')?.click();
		assert(fixture.devices.slice().length === 3, 'Forget removed a device without confirmation');
		page.querySelector<HTMLButtonElement>('.device-forget-confirm button:last-child')?.click();
		await settle();
		assert(fixture.devices.length === 2 && page.querySelector<HTMLElement>('.device-panel')?.hidden, 'Forget did not close the removed selection');
		const file = page.querySelector<HTMLInputElement>('input[type="file"]');
		assert(file, 'Invitation picker missing');
		const transfer = new DataTransfer();
		transfer.items.add(new File(['VNLF'], 'test.vnl'));
		file.files = transfer.files;
		file.dispatchEvent(new Event('change'));
		const deadline = Date.now() + 2000;
		while (page.querySelector<HTMLFormElement>('.device-pairing form')?.hidden || page.querySelector<HTMLButtonElement>('.device-pairing button[type="submit"]')?.disabled) {
			assert(Date.now() < deadline, 'Invitation file read did not settle');
			await new Promise<void>(resolve => {
				requestAnimationFrame(() => {
					resolve();
				});
			});
		}
		const code = page.querySelector<HTMLInputElement>('.device-pairing input[type="text"]');
		assert(code && !code.value && !page.querySelector('output')?.textContent, 'Import disclosed or filled the expected digits');
		code.value = '042';
		page.querySelector<HTMLFormElement>('.device-pairing form')?.dispatchEvent(new Event('submit', { cancelable: true }));
		await settle();
		assert(fixture.confirmation === '042' && page.querySelector('output')?.textContent === '731', `Pairing did not show reverse confirmation: ${fixture.confirmation}; ${page.querySelector('.device-pairing')?.textContent ?? ''}`);
	} finally {
		fixture.dispose();
	}
}

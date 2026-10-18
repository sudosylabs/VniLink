import { Emitter, Event as BaseEvent } from '../../src/base/common/event.js';
import { Disposable } from '../../src/base/common/lifecycle.js';
import type { ControlRequestInfo } from '../../src/platform/engine/common/engine.js';
import type { DeviceSummary, IDeviceService, PairingFile } from '../../src/workbench/services/devices/deviceService.js';
import type { ScreenAccess, ScreenSnapshotResult } from '../../src/workbench/services/screenSnapshot/screenSnapshot.js';
import type { IScreenSnapshotService } from '../../src/workbench/services/screenSnapshot/screenSnapshotService.js';
import { DevicesPage } from '../../src/workbench/contrib/devices/devicesPage.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { IDeviceService as IDeviceServiceId } from '../../src/workbench/services/devices/deviceService.js';
import { IScreenSnapshotService as IScreenSnapshotServiceId } from '../../src/workbench/services/screenSnapshot/screenSnapshotService.js';

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
	public readonly onDidChangeAvailability = BaseEvent.None;
	public devices: readonly DeviceSummary[] = [];
	public readonly page: DevicesPage;
	private readonly host = document.createElement('div');
	public connects: string[] = [];
	public confirmation = '';
	public renameResult: Promise<void> | undefined;
	public incoming: ControlRequestInfo | null = null;
	public readonly snapshots: FixtureSnapshots;

	constructor(state: DeviceFixtureState = 'populated', snapshots?: FixtureSnapshots) {
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
		this.snapshots = snapshots ?? new FixtureSnapshots();
		const collection = new ServiceCollection([IDeviceServiceId, this], [IScreenSnapshotServiceId, this.snapshots]);
		const services = this._register(new InstantiationService(collection, true));
		this.page = this._register(services.createInstance(DevicesPage));
		this.host.className = 'home-content device-fixture';
		this.host.style.cssText = 'position:fixed;top:var(--titlebar-height);left:190px;right:0;bottom:0;z-index:10;';
		this.host.append(this.page.element);
		document.body.append(this.host);
		if (state === 'selected') {
			this.page.element.querySelector<HTMLElement>('.device-row')?.click();
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

	public availability(): Promise<null> {
		return Promise.resolve(null);
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

const TINY_JPEG = new Uint8Array([0xff, 0xd8, 0xff, 0xd9]);

const STUDIO = 'a'.repeat(64);

function shot(capturedUnixMs: number, displayId: string, sessionId = '1', generation = '1'): ScreenSnapshotResult {
	return { status: 'ready', jpeg: TINY_JPEG, width: 1, height: 1, capturedUnixMs, displayId, endpointId: STUDIO, sessionId, generation };
}

/** Records screenshot requests. The default result is a synthetic JPEG, not a captured desktop. */
export class FixtureSnapshots implements IScreenSnapshotService {
	declare public readonly _serviceBrand: undefined;
	public calls: string[] = [];
	public prompts: boolean[] = [];
	public hold = false;
	public result: ScreenSnapshotResult = shot(Date.now(), 'fixture');
	public accessResult: ScreenAccess = { status: 'allowed', endpointId: '', sessionId: '1', generation: '1' };
	private pending: ((result: ScreenSnapshotResult) => void)[] = [];

	public capture(endpointId: string, signal: AbortSignal, prompt: boolean): Promise<ScreenSnapshotResult> {
		this.calls.push(endpointId);
		this.prompts.push(prompt);
		return new Promise(resolve => {
			signal.addEventListener('abort', () => {
				resolve({ status: 'cancelled' });
			}, { once: true });
			if (!this.hold) {
				resolve(this.result);
				return;
			}
			this.pending.push(resolve);
		});
	}

	public access(endpointId: string, signal: AbortSignal): Promise<ScreenAccess> {
		if (signal.aborted) {
			return Promise.resolve({ status: 'cancelled' });
		}
		const result = this.accessResult;
		if (result.status === 'allowed') {
			return Promise.resolve({ ...result, endpointId: result.endpointId || endpointId });
		}
		return Promise.resolve(result);
	}

	public release(result: ScreenSnapshotResult = this.result): void {
		this.pending.shift()?.(result);
	}
}

/** The list row that renders `row`; the list keeps focus and selection on these. */
function listRow(row: Element): HTMLElement {
	const element = row.closest<HTMLElement>('.monaco-list-row');
	assert(element, 'Device row is not inside a list row');
	return element;
}

function pressKey(target: Element, key: string, keyCode: number): void {
	target.dispatchEvent(new KeyboardEvent('keydown', { key, keyCode, bubbles: true }));
}

export async function verifyDevices(): Promise<void> {
	const fixture = new DevicePageFixture();
	const page = fixture.page.element;
	// List rows are recycled, so DOM order is not item order; address them by index.
	const rows = () => [0, 1, 2].map(index => page.querySelector<HTMLElement>(`.monaco-list-row[data-index="${String(index)}"] .device-row`)).filter((row): row is HTMLElement => row !== null);
	try {
		const tabCount = document.querySelectorAll('[role="tab"]').length;
		assert(rows().length === 3, 'Device fixture did not render');
		assert(!page.querySelector('.devices-toolbar'), 'Removed device actions are still presented');
		const list = page.querySelector<HTMLElement>('.device-list .monaco-list');
		assert(list?.getAttribute('role') === 'list' && listRow(rows()[0]).getAttribute('role') === 'listitem', 'Devices are not an accessible list');
		rows()[1].click();
		assert(!page.querySelector<HTMLElement>('.device-panel')?.hidden, 'Selecting a device did not open details');
		assert(document.querySelectorAll('[role="tab"]').length === tabCount, 'Device selection opened a titlebar tab');
		fixture.update(fixture.devices.map(device => device.id === 'b'.repeat(64) ? { ...device, presence: 'connecting' } : device));
		assert(listRow(rows()[1]).getAttribute('aria-selected') === 'true', 'Presence update lost selection');
		assert(rows()[1].textContent.includes('Connecting'), 'Presence update was not rendered');
		list.focus();
		pressKey(list, 'ArrowDown', 40);
		assert(listRow(rows()[2]).classList.contains('focused'), 'Arrow navigation failed');
		page.querySelector<HTMLButtonElement>('.device-close')?.click();
		assert(document.activeElement === list && listRow(rows()[1]).classList.contains('focused') && page.querySelector<HTMLElement>('.device-panel')?.hidden, 'Close did not restore list focus');
		fixture.update(fixture.devices.map(device => ({ ...device, presence: 'disconnected' })));
		rows()[1].click();
		page.querySelector<HTMLElement>('.device-panel .primary')?.click();
		await settle();
		assert(fixture.connects[0] === 'b'.repeat(64), 'Connect used the wrong endpoint');
		const name = page.querySelector<HTMLInputElement>('.device-rename input');
		assert(name, 'Rename controls missing');
		name.value = '<b>Personal</b>';
		name.dispatchEvent(new Event('input'));
		pressKey(name, 'Enter', 13);
		await settle();
		assert(rows()[1].textContent.includes('<b>Personal</b>') && !page.querySelector('b'), 'Rename rendered untrusted markup');
		let reject: (reason: Error) => void = () => {
			throw new Error('No pending request');
		};
		fixture.renameResult = new Promise<void>((_resolve, fail) => {
			reject = fail;
		});
		name.value = 'Pending';
		name.dispatchEvent(new Event('input'));
		pressKey(name, 'Enter', 13);
		rows()[0].click();
		reject(new Error('Old device failure'));
		await settle();
		assert(!page.querySelector('.device-panel')?.textContent.includes('Old device failure'), 'Late command error crossed selections');
		page.querySelector<HTMLElement>('.device-forget')?.click();
		assert(fixture.devices.slice().length === 3, 'Forget removed a device without confirmation');
		page.querySelector<HTMLElement>('.device-forget-confirm .monaco-button:last-child')?.click();
		await settle();
		assert(fixture.devices.length === 2 && page.querySelector<HTMLElement>('.device-panel')?.hidden, 'Forget did not close the removed selection');
		fixture.page.importInvitation(new File(['VNLF'], 'test.vnl'));
		const deadline = Date.now() + 2000;
		while (page.querySelector<HTMLElement>('.device-pairing-form')?.hidden || page.querySelector('.device-pairing-form .monaco-button.disabled')) {
			assert(Date.now() < deadline, 'Invitation file read did not settle');
			await new Promise<void>(resolve => {
				requestAnimationFrame(() => {
					resolve();
				});
			});
		}
		const code = page.querySelector<HTMLInputElement>('.device-pairing input[type="text"]');
		assert(code && !code.value && !page.querySelector('output')?.textContent, 'Import disclosed or filled the expected digits');
		code.value = '42';
		pressKey(code, 'Enter', 13);
		await settle();
		const confirmation = (): string => fixture.confirmation;
		assert(confirmation() === '' && page.querySelector('.device-pairing .device-feedback')?.textContent !== '', 'An incomplete code was submitted');
		code.value = '042';
		pressKey(code, 'Enter', 13);
		await settle();
		assert(confirmation() === '042' && page.querySelector('output')?.textContent === '731', `Pairing did not show reverse confirmation: ${confirmation()}; ${page.querySelector('.device-pairing')?.textContent ?? ''}`);
	} finally {
		fixture.dispose();
	}
}

export async function verifyDeviceScreenshot(): Promise<void> {
	const snapshots = new FixtureSnapshots();
	snapshots.hold = true;
	const fixture = new DevicePageFixture('populated', snapshots);
	const page = fixture.page.element;
	try {
		const rows = [0, 1, 2].map(index => page.querySelector<HTMLElement>(`.monaco-list-row[data-index="${String(index)}"] .device-row`)).filter((row): row is HTMLElement => row !== null);
		rows[1].click();
		assert(snapshots.calls.length === 0, 'A disconnected device requested a screenshot');
		assert(page.querySelector('.device-screen figcaption')?.textContent === 'Connect to see this computer’s screen.', 'Disconnected screenshot copy is wrong');
		rows[0].click();
		assert(snapshots.calls[0] === 'a'.repeat(64) && page.querySelector('.device-screen img')?.getAttribute('src') === null, 'The screenshot appeared before the remote image arrived');
		const refresh = page.querySelector<HTMLElement>('.device-screen .monaco-button');
		assert(refresh !== null && refresh.textContent === 'Refresh screen' && refresh.classList.contains('disabled'), 'Refresh was available while a screenshot was loading');
		rows[1].click();
		snapshots.release(shot(Date.now(), 'late'));
		await settle();
		assert(page.querySelector('.device-screen img')?.hasAttribute('src') !== true, 'A late screenshot from the first computer was shown on the second');
		const disconnectedCaption = page.querySelector('.device-screen figcaption');
		assert(disconnectedCaption !== null && !disconnectedCaption.textContent.toLowerCase().includes('asleep'), 'The panel invented a sleep state');
		rows[0].click();
		const capturedAt = Date.now() - 5_000;
		snapshots.release(shot(capturedAt, 'studio'));
		await settle();
		const image = page.querySelector<HTMLImageElement>('.device-screen img');
		assert(image && !image.hidden && image.alt === 'Screenshot of Studio Mac' && image.src.startsWith('blob:'), 'The connected device did not show its screenshot');
		assert(image.parentElement?.tagName !== 'BUTTON', 'The screenshot is an input surface');
		assert(page.querySelector('.device-screen figcaption')?.textContent === 'Screen captured just now.', 'Capture age does not describe the acquisition');
		const shown = image.src;
		rows[2].click();
		await settle();
		assert(page.querySelector<HTMLImageElement>('.device-screen img')?.hidden === true, 'Leaving the connected device kept its image');
		assert(shown.startsWith('blob:'), 'The test did not observe an object URL');
		rows[0].click();
		snapshots.release({ status: 'needsPermission' });
		await settle();
		assert(page.querySelector('.device-screen figcaption')?.textContent === 'That computer has not allowed screen capture.', 'Permission state was not shown');
		refresh.focus();
		assert(document.activeElement === page.querySelector('.device-screen .monaco-button'), 'Refresh cannot take keyboard focus');
		const status = page.querySelector('.device-screen-status');
		const age = page.querySelector('.device-screen-age');
		assert(status?.getAttribute('role') === 'status' && age && !age.hasAttribute('role') && !page.querySelector('.device-screen figcaption')?.hasAttribute('role'), 'Capture age is announced as a live status');
		const allow = [...page.querySelectorAll<HTMLElement>('.device-screen .monaco-button')].find(button => button.textContent === 'Allow screen capture');
		assert(allow && !allow.hidden, 'Allow screen capture was not offered');
		allow.click();
		assert(snapshots.prompts.at(-1) === true && page.querySelector('.device-screen .monaco-button')?.classList.contains('disabled') === true, 'Allow did not request consent');
		snapshots.release({ status: 'cancelled' });
		await settle();
		assert(page.querySelector('.device-screen .monaco-button')?.classList.contains('disabled') === false && page.querySelector('.device-screen figcaption')?.textContent === 'The screen could not be captured.', 'A cancelled screenshot left Refresh disabled');
		page.querySelector<HTMLElement>('.device-screen .monaco-button')?.click();
		snapshots.release({ status: 'needsSourceSelection' });
		await settle();
		const choose = [...page.querySelectorAll<HTMLElement>('.device-screen .monaco-button')].find(button => button.textContent === 'Choose screen');
		assert(choose && page.querySelector('.device-screen figcaption')?.textContent === 'Choose a screen on that computer, then refresh.', 'Choosing a screen was not offered');
		choose.click();
		assert(snapshots.prompts.at(-1) === true, 'Choose screen did not request a source');
		snapshots.release({ status: 'cancelled' });
		await settle();
		const callsBefore = snapshots.calls.length;
		fixture.update(fixture.devices.map(device => device.id === STUDIO ? { ...device, presence: 'disconnected' as const } : device));
		await settle();
		assert(page.querySelector<HTMLImageElement>('.device-screen img')?.hidden === true && page.querySelector('.device-screen figcaption')?.textContent === 'Connect to see this computer’s screen.', 'Disconnect kept the previous screen');
		fixture.update(fixture.devices.map(device => device.id === STUDIO ? { ...device, presence: 'connected' as const } : device));
		await settle();
		assert(snapshots.calls.length === callsBefore + 1 && snapshots.prompts.at(-1) === false, 'Returning to connected did not refresh without prompting');
		snapshots.release(shot(Date.now(), 'again'));
		await settle();
		assert(page.querySelector<HTMLImageElement>('.device-screen img')?.hidden === false, 'The reconnected computer did not show a new screenshot');
		const beforeReplace = snapshots.calls.length;
		snapshots.accessResult = { status: 'allowed', endpointId: STUDIO, sessionId: '2', generation: '9' };
		fixture.update(fixture.devices);
		await settle();
		assert(snapshots.calls.length === beforeReplace + 1 && snapshots.prompts.at(-1) === false && page.querySelector<HTMLImageElement>('.device-screen img')?.hidden === true, 'A replaced connection kept the previous screenshot');
		snapshots.release(shot(Date.now(), 'replaced', '2', '9'));
		await settle();
		assert(page.querySelector<HTMLImageElement>('.device-screen img')?.hidden === false, 'The replacement screenshot was not shown');
		snapshots.accessResult = { status: 'denied' };
		fixture.update(fixture.devices);
		await settle();
		assert(page.querySelector<HTMLImageElement>('.device-screen img')?.hidden === true && page.querySelector('.device-screen figcaption')?.textContent === 'Screen viewing is turned off on that computer.', 'A revoked view kept the screenshot');
	} finally {
		fixture.dispose();
	}
}

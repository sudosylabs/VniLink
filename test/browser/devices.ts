import { Emitter } from '../../src/base/common/event.js';
import { Disposable } from '../../src/base/common/lifecycle.js';
import type { ControlRequestInfo, RemoteCapabilityStatus } from '../../src/platform/engine/common/engine.js';
import type { DeviceSummary, IDeviceService, PairingFile } from '../../src/workbench/services/devices/deviceService.js';
import type { ScreenAccess, ScreenSnapshotResult } from '../../src/workbench/services/screenSnapshot/screenSnapshot.js';
import type { IScreenSnapshotService } from '../../src/workbench/services/screenSnapshot/screenSnapshotService.js';
import { DevicesPage } from '../../src/workbench/contrib/devices/devicesPage.js';
import { DevicePairingPopover, IDevicePairingPopover, type PairingPopoverKind } from '../../src/workbench/contrib/devices/devicePairingPopover.js';
import { SyncDescriptor } from '../../src/platform/instantiation/common/descriptors.js';
import { IDevicePairingService } from '../../src/workbench/services/devices/devicePairingService.js';
import { DevicePairingService } from '../../src/workbench/services/devices/devicePairingServiceImpl.js';
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
	private readonly availabilityChanged = this._register(new Emitter<string>());
	public readonly onDidChangeAvailability = this.availabilityChanged.event;
	public devices: readonly DeviceSummary[] = [];
	/** What each computer reports it allows; a missing entry has not reported. */
	public readonly reports = new Map<string, RemoteCapabilityStatus[]>();
	public readonly page: DevicesPage;
	public readonly popover: IDevicePairingPopover;
	public readonly popoverPairing: IDevicePairingService;
	private readonly host = document.createElement('div');
	/** Stand-ins for the titlebar buttons the popover hangs from. */
	private readonly anchors = document.createElement('div');
	public connects: string[] = [];
	public confirmation = '';
	public renameResult: Promise<void> | undefined;
	public incoming: ControlRequestInfo | null = null;
	public readonly snapshots: FixtureSnapshots;

	constructor(state: DeviceFixtureState = 'populated', snapshots?: FixtureSnapshots) {
		super();
		if (state === 'populated' || state === 'selected') {
			this.devices = [
				{ id: 'a'.repeat(64), name: 'Studio Mac', os: 'macos', presence: 'connected', hostname: 'studio.local', direct: 2, relays: 1 },
				{ id: 'b'.repeat(64), name: 'Sudosy', os: 'linux', presence: 'disconnected', hostname: null, direct: 0, relays: 1 },
				{ id: 'c'.repeat(64), name: 'Office PC', os: 'windows', presence: 'reconnecting', hostname: 'DESKTOP-7Q2M', direct: 1, relays: 0 },
			];
		} else if (state === 'unavailable') {
			this.state = 'unavailable';
		}
		this.snapshots = snapshots ?? new FixtureSnapshots();
		const collection = new ServiceCollection([IDeviceServiceId, this], [IScreenSnapshotServiceId, this.snapshots],
			[IDevicePairingService, new SyncDescriptor(DevicePairingService)], [IDevicePairingPopover, new SyncDescriptor(DevicePairingPopover)]);
		const services = this._register(new InstantiationService(collection, true));
		this.page = this._register(services.createInstance(DevicesPage));
		this.popover = services.invokeFunction(accessor => accessor.get(IDevicePairingPopover));
		this.popoverPairing = services.invokeFunction(accessor => accessor.get(IDevicePairingService));
		this.anchors.className = 'device-fixture-anchors';
		this.anchors.style.cssText = 'position:fixed;top:4px;right:8px;z-index:11;display:flex;';
		for (const kind of ['import', 'create'] as const) {
			const anchor = document.createElement('button');
			anchor.type = 'button';
			anchor.className = 'view-action';
			anchor.dataset.kind = kind;
			anchor.textContent = kind;
			anchor.addEventListener('click', () => {
				this.popover.toggle(anchor, kind);
			});
			this.anchors.append(anchor);
		}
		document.body.append(this.anchors);
		this.host.className = 'home-content device-fixture';
		// Mounted the way Home mounts a page that scrolls its own regions.
		this.host.classList.toggle('page-scrolls-itself', this.page.scrollsItself);
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

	public availability(id: string): Promise<readonly RemoteCapabilityStatus[] | null> {
		return Promise.resolve(this.reports.get(id) ?? null);
	}

	public report(id: string, report: RemoteCapabilityStatus[]): void {
		this.reports.set(id, report);
		this.availabilityChanged.fire(id);
	}

	/** The fixture's button for `kind`, standing in for the titlebar action. */
	public anchor(kind: PairingPopoverKind): HTMLButtonElement {
		const anchor = this.anchors.querySelector<HTMLButtonElement>(`[data-kind="${kind}"]`);
		assert(anchor, `No ${kind} anchor`);
		return anchor;
	}

	public update(devices: readonly DeviceSummary[]): void {
		this.devices = devices;
		this.changed.fire(devices);
	}

	public override dispose(): void {
		this.popover.hide();
		super.dispose();
		this.host.remove();
		this.anchors.remove();
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
		await verifyInspector(fixture, rows);
		fixture.update(fixture.devices.map(device => ({ ...device, presence: 'disconnected' })));
		rows()[1].click();
		assert(![...page.querySelectorAll('.device-panel button')].some(button => button.textContent === 'Connect'), 'The removed Connect button is still offered');
		const renameAction = (): HTMLButtonElement => {
			const button = page.querySelector<HTMLButtonElement>('.device-panel .device-text-button:not(.device-forget)');
			assert(button?.textContent === 'Rename…', 'Rename… is missing');
			return button;
		};
		assert(page.querySelector<HTMLElement>('.device-rename')?.hidden === true, 'The name field shows before Rename…');
		renameAction().click();
		assert(page.querySelector<HTMLElement>('.device-rename')?.hidden === false && page.querySelector<HTMLElement>('.device-panel h2')?.hidden === true, 'Rename… did not edit the name in place');
		const field = page.querySelector<HTMLInputElement>('.device-rename input');
		assert(field && document.activeElement === field && field.value === 'Sudosy', 'Rename did not focus the current name');
		pressKey(field, 'Escape', 27);
		assert(page.querySelector<HTMLElement>('.device-rename')?.hidden === true && !page.querySelector<HTMLElement>('.device-panel')?.hidden && document.activeElement === renameAction(), 'Escape in the name field closed the panel or lost focus');
		renameAction().click();
		const name = page.querySelector<HTMLInputElement>('.device-rename input');
		assert(name, 'Rename controls missing');
		name.value = '<b>Personal</b>';
		name.dispatchEvent(new Event('input'));
		pressKey(name, 'Enter', 13);
		await settle();
		assert(rows()[1].textContent.includes('<b>Personal</b>') && !page.querySelector('b'), 'Rename rendered untrusted markup');
		assert(page.querySelector<HTMLElement>('.device-rename')?.hidden === true, 'A saved name left the field open');
		renameAction().click();
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
	} finally {
		fixture.dispose();
	}
}

/** Read-only service rows, the details list and their updates for the selected computer. */
async function verifyInspector(fixture: DevicePageFixture, rows: () => HTMLElement[]): Promise<void> {
	const page = fixture.page.element;
	const services = (): Record<string, string> => Object.fromEntries([...page.querySelectorAll('.device-service')].map(row => [row.querySelector('.device-service-label')?.textContent ?? '', row.querySelector('.device-service-status')?.textContent ?? '']));
	fixture.report(STUDIO, [
		{ capability: 'desktopControl', status: 'available' }, { capability: 'desktopView', status: 'available' },
		{ capability: 'files', status: 'turnedOff' }, { capability: 'terminal', status: 'needsPermission' },
		{ capability: 'camera', status: 'unavailable' }, { capability: 'systemRestart', status: 'unsupported' }, { capability: 'systemShutdown', status: 'available' },
	]);
	rows()[0].click();
	await settle();
	const expected = {
		'Remote desktop': 'Available', 'View screen only': 'Available', 'Terminal': 'Needs permission on that computer', 'Files': 'Turned off by its owner',
		'Camera': 'Unavailable right now', 'Lock screen': 'Not supported there', 'Restart or shut down': 'Available',
	};
	assert(JSON.stringify(services()) === JSON.stringify(expected), `Service rows misreport availability: ${JSON.stringify(services())}`);
	assert(page.querySelector('.device-services')?.getAttribute('role') === 'list' && !page.querySelector('.device-service button, .device-service[tabindex]'), 'Service rows became interactive before their tabs exist');
	assert(page.querySelector('.device-service[data-status="turnedOff"]')?.getAttribute('aria-disabled') === 'true', 'An unavailable service is not marked disabled');
	fixture.report(STUDIO, [{ capability: 'files', status: 'available' }]);
	await settle();
	assert(services().Files === 'Available' && services()['Remote desktop'] === 'Not supported there', 'An availability change did not refresh the rows');
	const facts = Object.fromEntries([...page.querySelectorAll('.device-facts > div')].map(row => [row.querySelector('dt')?.textContent ?? '', row.querySelector('dd')?.textContent ?? '']));
	assert(facts['Device ID'] === 'aaaaaaaa…aaaaaa' && facts.Computer === 'studio.local' && facts.System === 'macOS' && facts.Addresses === '2 direct, 1 relay', `Details are wrong: ${JSON.stringify(facts)}`);
	assert(page.querySelector('.device-id')?.getAttribute('title') === STUDIO && page.querySelector('.device-copy')?.getAttribute('aria-label') === 'Copy device ID', 'The full device ID is not reachable');
	assert(page.querySelector('.device-panel-state')?.textContent === 'Connected · macOS', 'The identity line is wrong');
	fixture.update(fixture.devices.map(device => device.id === 'b'.repeat(64) ? { ...device, presence: 'disconnected' as const } : device));
	rows()[1].click();
	await settle();
	assert(Object.values(services()).every(status => status === 'Offline'), 'An offline computer reported stale services');
	assert(page.querySelector('.device-facts dd:not(:has(code))')?.textContent === 'Not reported', 'A missing hostname was invented');
	rows()[2].click();
	await settle();
	assert(Object.values(services()).every(status => status === 'Reconnecting') && [...page.querySelectorAll('.device-service')].every(row => row.getAttribute('aria-disabled') === 'true'), 'Services were not disabled while reconnecting');
}

async function until(condition: () => boolean, message: string): Promise<void> {
	const deadline = Date.now() + 3000;
	while (!condition()) {
		assert(Date.now() < deadline, message);
		await new Promise<void>(resolve => {
			requestAnimationFrame(() => {
				resolve();
			});
		});
	}
}

/** Pairing from the titlebar popovers: create, approve an incoming request, and open an invitation. */
export async function verifyPairingPopover(): Promise<void> {
	const fixture = new DevicePageFixture('empty');
	const popover = (): HTMLElement | null => document.querySelector('.device-pairing-popover');
	const text = (selector: string): string => popover()?.querySelector<HTMLElement>(`${selector}:not([hidden])`)?.textContent ?? '';
	const primary = (): HTMLElement | null => popover()?.querySelector<HTMLElement>('.device-pairing-foot .monaco-button.primary:not([hidden])') ?? null;
	const confirmation = (): string => fixture.confirmation;
	const digits = (): HTMLInputElement | null => popover()?.querySelector<HTMLInputElement>('.device-pairing-field:not([hidden]) input') ?? null;
	try {
		const create = fixture.anchor('create');
		create.click();
		await until(() => text('.device-pairing-code') === '042', 'The invitation code did not appear');
		const dialog = popover();
		assert(dialog?.getAttribute('role') === 'dialog' && dialog.getAttribute('aria-labelledby') === dialog.querySelector('h2')?.id && dialog.querySelector('h2')?.textContent === 'Invite another computer', 'The popover is not a labelled dialog');
		assert(/^Expires in 1[45]:[0-5][0-9]$/.test(text('.device-pairing-expiry')), `Expiry is missing: ${text('.device-pairing-expiry')}`);
		assert(primary()?.textContent === 'Save…' && dialog.contains(document.activeElement), 'Save… is missing or focus stayed outside the popover');
		pressKey(primary() ?? dialog, 'Escape', 27);
		assert(!popover() && document.activeElement === create, 'Escape did not close the popover and return focus');
		create.click();
		await until(() => text('.device-pairing-code') === '042', 'Reopening lost the live invitation');
		fixture.incoming = { endpoint_id: 'test-computer', check: '731' };
		await until(() => digits() !== null, 'An incoming request did not ask for its digits');
		assert(popover()?.querySelector('h2')?.textContent === 'Approve this computer' && !text('.device-pairing-code'), 'Approval disclosed the code or kept the invitation heading');
		fixture.incoming = null;
		await until(() => text('.device-pairing-code') === '042', 'A lapsed request did not return to the invitation');
		assert(text('.device-feedback') === 'The other computer stopped waiting. It will ask again while this invitation lasts.', 'A lapsed request was not explained');
		fixture.incoming = { endpoint_id: 'test-computer', check: '731' };
		await until(() => digits() !== null, 'The renewed request did not ask for its digits');
		const approve = digits();
		assert(approve && document.activeElement === approve, 'Approval did not focus the digits');
		approve.value = '731';
		pressKey(approve, 'Enter', 13);
		await until(() => text('.device-feedback').startsWith('Computer paired'), 'Approving did not pair');
		assert(confirmation() === '731', 'Approval sent the wrong digits');
		document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
		assert(!popover(), 'Pressing outside did not close the popover');
		const open = fixture.anchor('import');
		open.click();
		assert(popover()?.querySelector('h2')?.textContent === 'Pair with an invitation' && primary()?.textContent === 'Choose invitation…', 'A finished pairing was shown again instead of the chooser');
		await fixture.popoverPairing.openInvitation(new File(['VNLF'], 'office-pc.vnl'));
		await settle();
		const code = digits();
		assert(code && text('.device-pairing-file') === 'office-pc.vnl' && !text('.device-pairing-code'), 'Opening an invitation did not ask for the creator\'s digits');
		code.value = '42';
		pressKey(code, 'Enter', 13);
		await settle();
		assert(confirmation() === '731' && text('.device-feedback') === 'Enter the three digits shown on the other computer.', 'An incomplete code was submitted');
		code.value = '042';
		pressKey(code, 'Enter', 13);
		await until(() => text('.device-pairing-code') === '731', 'Pairing did not show the reverse code');
		assert(confirmation() === '042', 'Pairing sent the wrong digits');
		open.click();
		assert(!popover(), 'The anchor did not toggle its popover closed');
	} finally {
		fixture.dispose();
	}
	assert(!document.querySelector('.device-pairing-popover'), 'Disposing the fixture left the popover open');
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
		assert(page.querySelector('.device-screen figcaption')?.textContent === 'Offline. It may be asleep, off, or not running VniLink.', 'Offline screenshot copy is wrong');
		assert(page.querySelector<HTMLElement>('.device-screen-frame')?.dataset.state === 'offline' && page.querySelector<HTMLElement>('.device-screen-refresh')?.hidden === true, 'Offline showed a screen or a Refresh it cannot honour');
		rows[2].click();
		assert(page.querySelector('.device-screen figcaption')?.textContent === 'Reconnecting…' && page.querySelector<HTMLElement>('.device-screen-frame')?.dataset.state === 'connecting', 'Reconnecting was not shown');
		fixture.update(fixture.devices.map(device => device.id === 'c'.repeat(64) ? { ...device, presence: 'notAccepting' as const } : device));
		assert(page.querySelector('.device-screen figcaption')?.textContent === 'Its owner stopped accepting connections.' && page.querySelector<HTMLElement>('.device-screen-frame')?.dataset.state === 'notAccepting', 'Not accepting connections was not shown');
		assert(!snapshots.calls.length, 'An unreachable device requested a screenshot');
		rows[1].click();
		rows[0].click();
		assert(snapshots.calls[0] === 'a'.repeat(64) && page.querySelector('.device-screen img')?.getAttribute('src') === null, 'The screenshot appeared before the remote image arrived');
		const refresh = page.querySelector<HTMLButtonElement>('.device-screen-refresh');
		assert(refresh !== null && refresh.getAttribute('aria-label') === 'Refresh screen' && refresh.title === 'Refresh screen' && refresh.disabled && !refresh.hidden, 'Refresh was available while a screenshot was loading');
		rows[1].click();
		snapshots.release(shot(Date.now(), 'late'));
		await settle();
		assert(page.querySelector('.device-screen img')?.hasAttribute('src') !== true, 'A late screenshot from the first computer was shown on the second');
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
		assert(document.activeElement === refresh, 'Refresh cannot take keyboard focus');
		const status = page.querySelector('.device-screen-status');
		const age = page.querySelector('.device-screen-age');
		assert(status?.getAttribute('role') === 'status' && age && !age.hasAttribute('role') && !page.querySelector('.device-screen figcaption')?.hasAttribute('role'), 'Capture age is announced as a live status');
		const allow = [...page.querySelectorAll<HTMLElement>('.device-screen .monaco-button')].find(button => button.textContent === 'Allow screen capture');
		assert(allow && !allow.hidden, 'Allow screen capture was not offered');
		allow.click();
		assert(snapshots.prompts.at(-1) === true && page.querySelector('.device-screen .monaco-button')?.classList.contains('disabled') === true, 'Allow did not request consent');
		snapshots.release({ status: 'cancelled' });
		await settle();
		assert(page.querySelector<HTMLButtonElement>('.device-screen-refresh')?.disabled === false && page.querySelector('.device-screen figcaption')?.textContent === 'The screen could not be captured.', 'A cancelled screenshot left Refresh disabled');
		refresh.click();
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
		assert(page.querySelector<HTMLImageElement>('.device-screen img')?.hidden === true && page.querySelector('.device-screen figcaption')?.textContent === 'Offline. It may be asleep, off, or not running VniLink.', 'Disconnect kept the previous screen');
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

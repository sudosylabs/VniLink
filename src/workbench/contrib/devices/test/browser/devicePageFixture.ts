// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { $ } from '../../../../../base/browser/dom.js';
import { Emitter, Event } from '../../../../../base/common/event.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import { IClipboardService } from '../../../../../platform/clipboard/common/clipboardService.js';
import { ICommandService, type ICommandEvent } from '../../../../../platform/commands/common/commands.js';
import { ContextKeyService } from '../../../../../platform/contextkey/browser/contextKeyService.js';
import { IContextKeyService, type IContextKey } from '../../../../../platform/contextkey/common/contextkey.js';
import type { ControlRequestInfo, RemoteCapabilityStatus } from '../../../../../platform/engine/common/engine.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { EngineAvailableContext } from '../../../../contextkeys.js';
import { TestConfigurationService } from '../../../../services/configuration/test/common/testConfigurationService.js';
import { IDeviceService, type ControlCheck, type DeviceSummary, type PairingFile } from '../../../../services/devices/deviceService.js';
import { IDevicePairingService } from '../../../../services/devices/devicePairingService.js';
import { DevicePairingService } from '../../../../services/devices/devicePairingServiceImpl.js';
import type { ScreenAccess, ScreenSnapshotResult } from '../../../../services/screenSnapshot/screenSnapshot.js';
import { IScreenSnapshotService } from '../../../../services/screenSnapshot/screenSnapshotService.js';
import { ISystemActionService } from '../../../../services/systemActions/systemActionService.js';
import { TestSystemActionService } from '../../../../services/systemActions/test/common/testSystemActionService.js';
import { TestClipboardService } from '../../../../test/common/workbenchTestServices.js';
import { DevicePairingPopover, IDevicePairingPopover, type PairingPopoverKind } from '../../devicePairingPopover.js';
import { DevicesPage } from '../../devicesPage.js';

/** The computers a populated page lists: one connected, one disconnected, one reconnecting. */
export const STUDIO = 'a'.repeat(64);

export const LAB = 'b'.repeat(64);

export const OFFICE = 'c'.repeat(64);

export type DeviceFixtureState = 'empty' | 'populated' | 'selected' | 'unavailable';

const TINY_JPEG = new Uint8Array([0xff, 0xd8, 0xff, 0xd9]);

/** A synthetic screenshot of Studio, not a captured desktop. */
export function screenshot(capturedUnixMs: number, displayId: string, sessionId = '1', generation = '1'): ScreenSnapshotResult {
	return { status: 'ready', jpeg: TINY_JPEG, width: 1, height: 1, capturedUnixMs, displayId, endpointId: STUDIO, sessionId, generation };
}

/**
 * An {@link IScreenSnapshotService} that records each request and whether it
 * prompted. It answers with `result` at once, or, while `hold` is set, when
 * the test calls `release`; `accessResult` is what `access` reports.
 */
export class TestScreenSnapshotService implements IScreenSnapshotService {
	declare public readonly _serviceBrand: undefined;
	public readonly calls: string[] = [];
	public readonly prompts: boolean[] = [];
	public hold = false;
	public result: ScreenSnapshotResult = screenshot(Date.now(), 'fixture');
	public accessResult: ScreenAccess = { status: 'allowed', endpointId: '', sessionId: '1', generation: '1' };
	private readonly pending: ((result: ScreenSnapshotResult) => void)[] = [];

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

	/** Answers the oldest held request with `result`. */
	public release(result: ScreenSnapshotResult = this.result): void {
		this.pending.shift()?.(result);
	}
}

/** An {@link ICommandService} that runs nothing and records each command with its arguments. */
export class TestCommandService implements ICommandService {
	declare public readonly _serviceBrand: undefined;
	public readonly onWillExecuteCommand = Event.None as Event<ICommandEvent>;
	public readonly onDidExecuteCommand = Event.None as Event<ICommandEvent>;
	public readonly commands: unknown[][] = [];

	public executeCommand<R>(id: string, ...args: unknown[]): Promise<R | undefined> {
		this.commands.push([id, ...args]);
		return Promise.resolve(undefined);
	}
}

/**
 * The Devices page mounted as Home mounts it, over this fixture standing in
 * for the device service: `update` and `report` deliver what the engine would
 * report, `renameResult` holds a rename open, and `incoming` is the pending
 * pairing request. Two buttons stand in for the titlebar's pairing buttons
 * and open the real pairing popover. Disposing removes everything it showed.
 */
export class DevicePageFixture extends Disposable implements IDeviceService {
	declare public readonly _serviceBrand: undefined;
	public readonly available = true;
	public state: 'loading' | 'ready' | 'unavailable' = 'ready';
	private readonly changed = this._register(new Emitter<readonly DeviceSummary[]>());
	public readonly onDidChangeDevices = this.changed.event;
	public readonly onDidChangeState = this._register(new Emitter<void>()).event;
	private readonly availabilityChanged = this._register(new Emitter<string>());
	public readonly onDidChangeAvailability = this.availabilityChanged.event;
	private readonly checked = this._register(new Emitter<ControlCheck>());
	public readonly onDidReceiveControlCheck = this.checked.event;
	public devices: readonly DeviceSummary[] = [];
	/** What each computer reports it allows; a missing entry has not reported. */
	private readonly reports = new Map<string, RemoteCapabilityStatus[]>();
	public readonly page: DevicesPage;
	public readonly popover: IDevicePairingPopover;
	public readonly popoverPairing: IDevicePairingService;
	public readonly clipboard = new TestClipboardService();
	public readonly commandService = new TestCommandService();
	public readonly systemActions = this._register(new TestSystemActionService());
	public readonly engineAvailable: IContextKey<boolean>;
	/** The digits the last pairing or approval sent. */
	public confirmation = '';
	public renameResult: Promise<void> | undefined;
	public incoming: ControlRequestInfo | null = null;
	/** The operating system's name for this computer. */
	public systemName: string | null = 'workstation';
	private readonly host = $('div');
	private readonly anchors = $('div');

	constructor(state: DeviceFixtureState = 'populated', public readonly snapshots = new TestScreenSnapshotService()) {
		super();
		if (state === 'populated' || state === 'selected') {
			this.devices = [
				{ id: STUDIO, name: 'Studio Mac', os: 'macos', presence: 'connected', hostname: 'studio.local', direct: 2, relays: 1 },
				{ id: LAB, name: 'Lab Laptop', os: 'linux', presence: 'disconnected', hostname: null, direct: 0, relays: 1 },
				{ id: OFFICE, name: 'Office PC', os: 'windows', presence: 'reconnecting', hostname: 'DESKTOP-7Q2M', direct: 1, relays: 0 },
			];
		} else if (state === 'unavailable') {
			this.state = 'unavailable';
		}
		const instantiationService = this._register(new TestInstantiationService());
		const contextKeys = this._register(new ContextKeyService(this._register(new TestConfigurationService())));
		this.engineAvailable = EngineAvailableContext.bindTo(contextKeys);
		this.engineAvailable.set(true);
		instantiationService.stub(IDeviceService, this);
		instantiationService.stub(IScreenSnapshotService, this.snapshots);
		instantiationService.stub(ICommandService, this.commandService);
		instantiationService.stub(IContextKeyService, contextKeys);
		instantiationService.stub(ISystemActionService, this.systemActions);
		instantiationService.stub(IClipboardService, this.clipboard);
		this.popoverPairing = this._register(instantiationService.createInstance(DevicePairingService));
		instantiationService.stub(IDevicePairingService, this.popoverPairing);
		const popover = this._register(instantiationService.createInstance(DevicePairingPopover));
		this.popover = popover;
		instantiationService.stub(IDevicePairingPopover, popover);
		this.page = this._register(instantiationService.createInstance(DevicesPage));
		this.anchors.className = 'device-fixture-anchors';
		this.anchors.style.cssText = 'position:fixed;top:4px;right:8px;z-index:11;display:flex;';
		for (const kind of ['import', 'create'] as const) {
			const anchor = $('button', { type: 'button' });
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
			this.page.element.querySelector('.device-row')?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
		}
	}

	/** Commands the page ran, with their arguments. */
	public get commands(): readonly unknown[][] {
		return this.commandService.commands;
	}

	/** The row showing the device at `index` of the list; rows are recycled, so their DOM order is not the list's. */
	public row(index: number): HTMLElement {
		const row = this.page.element.querySelector<HTMLElement>(`.monaco-list-row[data-index="${String(index)}"] .device-row`);
		if (!row) {
			throw new Error(`No row at ${String(index)}`);
		}
		return row;
	}

	public initialize(): Promise<void> {
		return Promise.resolve();
	}

	public refresh(): Promise<void> {
		return Promise.resolve();
	}

	public connect(id: string): Promise<void> {
		this.checked.fire({ endpointId: id, check: '918273' });
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
		return Promise.resolve({ bytes: [86, 78, 76, 70], check: '042517', expiresAt: Date.now() + 900000 });
	}

	public pair(_bytes: Uint8Array, digits: string): Promise<string> {
		this.confirmation = digits;
		return Promise.resolve(LAB);
	}

	public incomingPairing(): Promise<ControlRequestInfo | null> {
		return Promise.resolve(this.incoming);
	}

	public systemComputerName(): Promise<string | null> {
		return Promise.resolve(this.systemName);
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

	/** Delivers what computer `id` now reports it allows. */
	public report(id: string, report: RemoteCapabilityStatus[]): void {
		this.reports.set(id, report);
		this.availabilityChanged.fire(id);
	}

	/** The fixture's button for `kind`, standing in for the titlebar's pairing button. */
	public anchor(kind: PairingPopoverKind): HTMLButtonElement {
		const anchor = this.anchors.querySelector<HTMLButtonElement>(`[data-kind="${kind}"]`);
		if (!anchor) {
			throw new Error(`No ${kind} anchor`);
		}
		return anchor;
	}

	/** Delivers a new device list. */
	public update(devices: readonly DeviceSummary[]): void {
		this.devices = devices;
		this.changed.fire(devices);
	}

	/** Changes the presence of device `id` and delivers the list. */
	public setPresence(id: string, presence: DeviceSummary['presence']): void {
		this.update(this.devices.map(device => device.id === id ? { ...device, presence } : device));
	}

	public override dispose(): void {
		this.popover.hide();
		super.dispose();
		this.host.remove();
		this.anchors.remove();
	}
}

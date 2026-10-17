import * as nls from '../../../nls.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../base/browser/domEvent.js';
import { Icons } from '../../../base/common/icons.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { IDeviceService } from '../../services/devices/deviceService.js';
import { IScreenSnapshotService } from '../../services/screenSnapshot/screenSnapshotService.js';
import type { TabAction } from '../../services/tabs/tabService.js';
import { createPageHeading, type WorkbenchPage } from '../../parts/views/page.js';
import { DeviceList } from './deviceList.js';
import { DeviceDetails } from './deviceDetails.js';
import { DevicePairing } from './devicePairing.js';
import './media/devices.css';

/** Composes known-device selection and pairing; service views own their own tabs. */
export class DevicesPage extends Disposable implements WorkbenchPage {
	private static nextId = 0;
	public readonly element = document.createElement('main');
	public readonly actions: readonly TabAction[] = [];
	private readonly body = document.createElement('div');
	private readonly empty = document.createElement('section');
	private readonly emptyTitle = document.createElement('h2');
	private readonly emptyHelp = document.createElement('p');
	private readonly feedback = document.createElement('p');
	private readonly list: DeviceList;
	private readonly details: DeviceDetails;
	private readonly pairing: DevicePairing;
	private selectedId: string | undefined;

	constructor(
		@IDeviceService private readonly devices: IDeviceService,
		@IScreenSnapshotService snapshots: IScreenSnapshotService,
	) {
		super();
		const id = `device-details-${String(DevicesPage.nextId++)}`;
		this.list = this._register(new DeviceList(id));
		this.details = this._register(new DeviceDetails(id, devices, snapshots));
		this.pairing = this._register(new DevicePairing(devices));
		this.element.className = 'devices-page';
		const heading = createPageHeading(nls.localize('computers.heading', 'Devices'), nls.localize('computers.subheading', 'Your computers, connected.'));
		this.element.tabIndex = -1;
		this.empty.className = 'device-empty';
		this.empty.append(renderIcon(Icons.devices), this.emptyTitle, this.emptyHelp);
		this.body.className = 'devices-body';
		const column = document.createElement('div');
		column.className = 'devices-list-column';
		column.append(this.list.element, this.empty);
		this.feedback.className = 'device-feedback';
		this.feedback.setAttribute('role', 'status');
		this.body.append(column, this.details.element);
		this.element.append(heading, this.pairing.element, this.feedback, this.body);
		this._register(this.list.onDidSelect(id => {
			this.selectedId = id;
			this.render();
		}));
		this._register(this.details.onDidClose(() => {
			const id = this.selectedId;
			this.selectedId = undefined;
			this.render();
			this.list.focus(id);
		}));
		this._register(devices.onDidChangeState(() => {
			this.render();
		}));
		this._register(devices.onDidChangeDevices(() => {
			this.render();
		}));
		this._register(addDisposableListener(this.element, 'keydown', event => {
			if ((event as KeyboardEvent).key === 'Escape' && this.selectedId) {
				const id = this.selectedId;
				this.selectedId = undefined;
				this.render();
				this.list.focus(id);
			}
		}));
		this.render();
		void devices.initialize().catch(() => {
			if (!this._store.isDisposed) {
				this.feedback.textContent = nls.localize('devices.loadFailed', 'Unable to load devices. Restart VniLink to try again.');
			}
		});
	}

	public importInvitation(): void {
		this.pairing.importButton.click();
	}

	public shareInvitation(): void {
		this.pairing.shareButton.click();
	}

	private render(): void {
		const state = this.devices.state;
		this.emptyTitle.textContent = state === 'loading' ? nls.localize('devices.loading', 'Loading computers…') : state === 'unavailable' ? nls.localize('devices.unavailable', 'Devices unavailable') : nls.localize('devices.emptyTitle', 'Bring your computers together');
		this.emptyHelp.textContent = state === 'unavailable' ? nls.localize('devices.unavailableHelp', 'Unable to read devices from the connection engine. Restart VniLink to try again.') : state === 'loading' ? '' : nls.localize('devices.emptyHelp', 'Import a .vnl invitation from another computer, or share one from this computer to get started.');
		this.empty.setAttribute('role', 'status');
		const selected = this.devices.devices.find(device => device.id === this.selectedId);
		const lostFocus = !!this.selectedId && !selected && this.details.element.contains(document.activeElement);
		this.selectedId = selected?.id;
		this.list.update(this.devices.devices, this.selectedId);
		this.details.update(selected);
		this.body.classList.toggle('has-selection', !!selected);
		this.empty.hidden = this.devices.devices.length > 0;
		this.list.element.hidden = !this.devices.devices.length;
		if (lostFocus) {
			if (this.devices.devices.length) {
				this.list.focus();
			} else {
				this.element.focus();
			}
		}
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}

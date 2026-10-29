import './media/devices.css';
import * as nls from '../../../nls.js';
import { addDisposableListener, append, EventType, h } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { IDeviceService } from '../../services/devices/deviceService.js';
import { createPageHeading, type IHomePage } from '../home/homePages.js';
import { DeviceDetails } from './deviceDetails.js';
import { DeviceList } from './deviceList.js';

/**
 * Home's Devices page: remembered computers beside the selected one's
 * details. Pairing lives in the titlebar actions this page shows.
 */
export class DevicesPage extends Disposable implements IHomePage {
	private static nextId = 0;
	public readonly element: HTMLElement;
	/** The list and the inspector scroll on their own, so the inspector stays a full-height sheet. */
	public readonly scrollsItself = true;
	private readonly main: HTMLElement;
	private readonly empty: HTMLElement;
	private readonly emptyTitle: HTMLElement;
	private readonly emptyHelp: HTMLElement;
	private readonly feedback: HTMLElement;
	private readonly list: DeviceList;
	private readonly details: DeviceDetails;
	private selectedId: string | undefined;

	constructor(
		@IDeviceService private readonly devices: IDeviceService,
		@IInstantiationService instantiationService: IInstantiationService,
	) {
		super();
		const detailsId = `device-details-${String(DevicesPage.nextId++)}`;
		this.list = this._register(new DeviceList(detailsId));
		this.details = this._register(instantiationService.createInstance(DeviceDetails, detailsId));
		// The page is the size container; the grid inside it can then change with the width.
		const page = h('main.devices-page', { tabindex: '-1' }, [
			h('div.devices-layout@layout', [
				h('div.devices-main@main', [
					h('p.device-feedback@feedback', { role: 'status' }),
					h('div.devices-list-column@column', [
						h('section.device-empty@empty', { role: 'status' }, [h('h2@emptyTitle'), h('p@emptyHelp')]),
					]),
				]),
			]),
		]);
		this.element = page.root;
		this.main = page.main;
		this.empty = page.empty;
		this.emptyTitle = page.emptyTitle;
		this.emptyHelp = page.emptyHelp;
		this.feedback = page.feedback;
		this.empty.prepend(renderIcon(Icons.devices));
		page.column.prepend(this.list.element);
		append(page.layout, this.details.element);
		this.main.prepend(createPageHeading(nls.localize('computers.heading', 'Devices'), nls.localize('computers.subheading', 'Your computers, connected.')));
		this.registerListeners();
		this.render();
		this.devices.initialize().catch(() => {
			if (!this._store.isDisposed) {
				this.feedback.textContent = nls.localize('devices.loadFailed', 'Unable to load devices. Restart VniLink to try again.');
			}
		});
	}

	private registerListeners(): void {
		this._register(this.list.onDidSelect(device => {
			this.selectedId = device.id;
			this.render();
		}));
		this._register(this.details.onDidClose(() => {
			this.closeDetails();
		}));
		this._register(this.devices.onDidChangeState(() => {
			this.render();
		}));
		this._register(this.devices.onDidChangeDevices(() => {
			this.render();
		}));
		this._register(addDisposableListener(this.element, EventType.KEY_DOWN, event => {
			if (event.key === 'Escape' && this.selectedId) {
				this.closeDetails();
			}
		}));
	}

	private closeDetails(): void {
		const id = this.selectedId;
		this.selectedId = undefined;
		this.render();
		this.list.focus(id);
	}

	private render(): void {
		const state = this.devices.state;
		this.emptyTitle.textContent = state === 'loading' ? nls.localize('devices.loading', 'Loading computers…')
			: state === 'unavailable' ? nls.localize('devices.unavailable', 'Devices unavailable')
				: nls.localize('devices.emptyTitle', 'Bring your computers together');
		this.emptyHelp.textContent = state === 'unavailable' ? nls.localize('devices.unavailableHelp', 'Unable to read devices from the connection engine. Restart VniLink to try again.')
			: state === 'loading' ? ''
				: nls.localize('devices.emptyHelp', 'Import a .vnl invitation from another computer, or share one from this computer to get started.');
		const known = this.devices.devices;
		const selected = known.find(device => device.id === this.selectedId);
		const lostFocus = !!this.selectedId && !selected && this.details.element.contains(document.activeElement);
		this.selectedId = selected?.id;
		this.list.update(known, this.selectedId);
		this.details.update(selected);
		this.element.classList.toggle('has-selection', !!selected);
		this.empty.hidden = known.length > 0;
		this.list.element.hidden = !known.length;
		if (lostFocus) {
			if (known.length) {
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

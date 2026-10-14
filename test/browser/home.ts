import { IApplicationService } from '../../src/workbench/services/application/common/applicationService.js';
import { IConnectionService } from '../../src/workbench/services/connection/common/connectionService.js';
import { IConfigurationService, defaultConfiguration } from '../../src/platform/configuration/common/configuration.js';
import { Event, Emitter } from '../../src/base/common/event.js';
import { HomeView } from '../../src/workbench/browser/features/home/home.js';
import { IDeviceService, type DeviceSummary } from '../../src/workbench/services/devices/common/deviceService.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';

function assert(condition: boolean, message: string): void {
	if (!condition) {
		throw new Error(message);
	}
}

export async function verifyHomeLifecycle(): Promise<void> {
	const viewport = document.createElement('div');
	viewport.className = 'home-content';
	viewport.style.cssText = 'position: fixed; top: 0; left: 0; width: 600px; height: 160px;';
	const page = document.createElement('main');
	const last = document.createElement('div');
	last.style.height = '600px';
	page.append(last);
	viewport.append(page);
	document.body.append(viewport);
	try {
		viewport.scrollTop = viewport.scrollHeight;
		const bottomGap = viewport.getBoundingClientRect().bottom - last.getBoundingClientRect().bottom;
		assert(bottomGap >= 47, 'Long pages overflowed past their bottom padding');
		viewport.style.width = '1400px';
		const bounds = page.getBoundingClientRect();
		const container = viewport.getBoundingClientRect();
		assert(Math.abs((bounds.left - container.left) - (container.left + viewport.clientWidth - bounds.right)) <= 1, 'Wide pages are not centered');
	} finally {
		viewport.remove();
	}
	const changed = new Emitter<readonly DeviceSummary[]>();
	let devices: readonly DeviceSummary[] = [];
	let requests = 0;
	let restartRequired = false;
	let restarts = 0;
	const restartChanged = new Emitter<void>();
	const service: IDeviceService = {
		_serviceBrand: undefined,
		available: true,
		get devices() {
			return devices;
		},
		onDidChangeDevices: changed.event,
		refresh() {
			requests++;
			devices = [{ id: 'peer', name: '<b>Remote computer</b>' }];
			changed.fire(devices);
			return Promise.resolve();
		},
	};
	const services = new InstantiationService(new ServiceCollection([IApplicationService, {
		_serviceBrand: undefined, get restartRequired() {
			return restartRequired;
		}, onDidChangeRestartRequired: restartChanged.event, restart: () => {
			restarts++;
			return Promise.resolve();
		}, openRelayDocumentation: () => Promise.resolve()
	}], [IDeviceService, service], [IConnectionService, { _serviceBrand: undefined, available: false, load: () => Promise.reject(new Error('Unavailable')), save: () => Promise.reject(new Error('Unavailable')) }], [IConfigurationService, { _serviceBrand: undefined, snapshot: { revision: 0, values: defaultConfiguration, error: null }, onDidChangeConfiguration: Event.None, initialize: () => Promise.resolve(), updateValue: () => Promise.resolve() }]), true);
	const home = services.createInstance(HomeView);
	try {
		assert(!home.element.querySelector('[data-section-id="files"]'), 'Files settings still appear in the sidebar');
		const devicesPage = home.element.querySelector('.home-content > main');
		const connection = home.element.querySelector<HTMLButtonElement>('[data-section-id="connection"]');
		const back = home.element.querySelector<HTMLButtonElement>('[data-section-id="devices"]');
		assert(!!devicesPage && !!connection && !!back, 'Home failed to create its pages through DI');
		if (!devicesPage || !connection || !back) {
			return;
		}
		const refresh = home.actions[0];
		connection.click();
		const settingsPage = home.element.querySelector('.home-content > main');
		assert(home.actions.length === 0, 'Settings retained device actions');
		restartRequired = true;
		restartChanged.fire();
		assert(home.actions.some(action => action.id === 'home.restart'), 'Restart action missing on settings page');
		await home.actions[0].run(new AbortController().signal);
		assert(restarts === 1, 'Restart action bypassed application service');
		restartRequired = false;
		restartChanged.fire();
		assert(home.actions.length === 0, 'Restart action survived reverted configuration');
		await refresh.run(new AbortController().signal);
		assert(requests === 1, 'Refresh bypassed the device service');
		back.click();
		assert(home.element.querySelector('.home-content > main') === devicesPage, 'Devices page was recreated');
		assert((devicesPage.textContent.includes('<b>Remote computer</b>')) && !devicesPage.querySelector('b'), 'Device data became HTML');
		connection.click();
		assert(home.element.querySelector('.home-content > main') === settingsPage, 'Settings page was recreated');
		assert(changed.hasListeners(), 'Devices page did not subscribe to state');
		home.dispose();
		assert(!changed.hasListeners(), 'Disposing Home left a device listener');
		back.click();
		changed.fire([]);
		assert(devicesPage.textContent.includes('<b>Remote computer</b>'), 'Disposed page still renders changes');
	} finally {
		home.dispose();
		services.dispose();
		changed.dispose();
		restartChanged.dispose();
	}
}

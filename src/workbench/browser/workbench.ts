import './styles/workbench.css';
import type { AppInfo, DesktopApi } from '../../platform/ipc/common/desktopApi.js';
import { createWorkbenchServices } from '../electron-browser/desktop.main.js';
import { NativeTitlebarPart } from '../electron-browser/parts/titlebar/titlebarPart.js';
import { ensureCodeWindow } from '../../base/browser/window.js';
import { createSidebar } from './parts/sidebar/sidebar.js';
import { createComputersPage } from './features/computers/computers.js';
export async function renderWorkbench(info: AppInfo, api: DesktopApi): Promise<void> {
	document.documentElement.dataset.platform = info.platform;
	const status = document.createElement('footer'); status.className = 'status'; status.setAttribute('role', 'status');
	status.textContent = info.engineConnected ? `Engine ready · ${info.version}` : 'Engine unavailable — secure storage could not be opened';
	function showError(error: unknown): void {
		console.error(error);
		status.textContent = 'The desktop could not complete the action. Close the window and try again.';
	}
	const workspace = document.createElement('div'); workspace.className = 'workspace';
	const page = createComputersPage(); page.append(status);
	workspace.append(createSidebar(), page);
	ensureCodeWindow(window, info.windowId);
	const services = createWorkbenchServices(api, info);
	const titlebar = services.createInstance(NativeTitlebarPart, info.osRelease, showError);
	const header = document.createElement('header');
	titlebar.create(header);
	document.body.replaceChildren(header, workspace);
	window.addEventListener('pagehide', () => {
		titlebar.dispose(); services.dispose();
	}, { once: true });
	await titlebar.initialize();
	document.documentElement.dataset.workbenchReady = 'true';
	if (info.engineConnected) {
		void api.engineCommand({ type: 'snapshot' }).then(reply => {
			if (reply.type !== 'snapshot') {
				throw new Error('Expected engine snapshot');
			}
			const state = page.querySelector('.empty-state');
			if (state && reply.snapshot.devices.length > 0) {
				state.replaceChildren();
				for (const device of reply.snapshot.devices) {
					const label = document.createElement('p');
					label.textContent = device.pet_name ?? device.hostname ?? device.endpoint_id;
					state.append(label);
				}
			}
		}).catch(showError);
	}
}

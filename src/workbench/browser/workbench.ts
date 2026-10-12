import { ITabService } from '../services/tabs/common/tabService.js';
import { ViewHost } from './parts/views/viewHost.js';
import './styles/workbench.css';
import type { AppInfo, DesktopApi } from '../../platform/ipc/common/desktopApi.js';
import { createWorkbenchServices } from '../electron-browser/desktop.main.js';
import { NativeTitlebarPart } from '../electron-browser/parts/titlebar/titlebarPart.js';
import { ensureCodeWindow } from '../../base/browser/window.js';
import { ComputersView } from './features/computers/computers.js';
export async function renderWorkbench(info: AppInfo, api: DesktopApi): Promise<void> {
	document.documentElement.dataset.platform = info.platform;
	const status = document.createElement('footer');
	status.className = 'status';
	status.setAttribute('role', 'status');
	status.textContent = info.engineConnected
		? `Engine ready · ${info.version}`
		: 'Engine unavailable — secure storage could not be opened';
	function showError(error: unknown): void {
		console.error(error);
		status.textContent = 'The desktop could not complete the action. Close the window and try again.';
	}
	const workspace = document.createElement('div');
	workspace.className = 'workspace';
	ensureCodeWindow(window, info.windowId);
	const services = createWorkbenchServices(api, info);
	const titlebar = services.createInstance(NativeTitlebarPart, info.osRelease, showError);
	const tabs = services.invokeFunction((accessor) => accessor.get(ITabService));
	const viewsContainer = document.createElement('div');
	const viewHost = services.createInstance(ViewHost, viewsContainer);
	const openComputers = (): void => {
		if (tabs.tabs.some((tab) => tab.id === 'computers')) {
			tabs.select('computers');
		} else {
			viewHost.open(new ComputersView(api, info.engineConnected));
		}
		if (info.engineConnected) {
			void tabs.executeAction('refresh').catch(showError);
		}
	};
	const content = document.createElement('div');
	content.className = 'view-stack';
	content.append(viewsContainer, status);
	workspace.append(content);
	openComputers();
	const header = document.createElement('header');
	titlebar.create(header);
	document.body.replaceChildren(header, workspace);
	window.addEventListener(
		'pagehide',
		() => {
			titlebar.dispose();
			viewHost.dispose();
			services.dispose();
		},
		{ once: true },
	);
	await titlebar.initialize();
	document.documentElement.dataset.workbenchReady = 'true';
}

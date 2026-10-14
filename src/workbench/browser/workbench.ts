import * as nls from '../../nls.js';
import { ITabService } from '../services/tabs/common/tabService.js';
import { ViewHost } from './parts/views/viewHost.js';
import './styles/workbench.css';
import type { AppInfo, DesktopApi } from '../../platform/ipc/common/desktopApi.js';
import { createWorkbenchServices } from '../electron-browser/desktop.main.js';
import { NativeTitlebarPart } from '../electron-browser/parts/titlebar/titlebarPart.js';
import { ensureCodeWindow } from '../../base/browser/window.js';
import { HomeView } from './features/home/home.js';
import { IConfigurationService } from '../../platform/configuration/common/configuration.js';

export async function renderWorkbench(info: AppInfo, api: DesktopApi): Promise<void> {
	document.documentElement.dataset.platform = info.platform;
	const notice = document.createElement('div');
	notice.className = 'workbench-notice';
	notice.setAttribute('role', 'alert');
	notice.hidden = info.engineConnected;
	notice.textContent = nls.localize('workbench.engineUnavailable', 'VniLink could not start its connection service. Restart the app to try again.');
	function showError(error: unknown): void {
		console.error(error);
		notice.hidden = false;
		notice.textContent = nls.localize('workbench.actionFailed', 'The action could not be completed. Restart the app to try again.');
	}
	const workspace = document.createElement('div');
	workspace.className = 'workspace';
	ensureCodeWindow(window, info.windowId);
	const services = createWorkbenchServices(api, info);
	await services.invokeFunction(accessor => accessor.get(IConfigurationService)).initialize();
	const titlebar = services.createInstance(NativeTitlebarPart, info.osRelease, showError);
	const tabs = services.invokeFunction((accessor) => accessor.get(ITabService));
	const viewsContainer = document.createElement('div');
	const viewHost = services.createInstance(ViewHost, viewsContainer);
	const openHome = (): void => {
		if (tabs.tabs.some((tab) => tab.id === 'home')) {
			tabs.select('home');
		} else {
			viewHost.open(services.createInstance(HomeView));
		}
		if (info.engineConnected) {
			void tabs.executeAction('refresh').catch(showError);
		}
	};
	const content = document.createElement('div');
	content.className = 'view-stack';
	content.append(viewsContainer);
	workspace.append(notice, content);
	openHome();
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

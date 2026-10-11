import './styles/workbench.css';
import type { AppInfo, DesktopApi } from '../../platform/ipc/common/desktopApi';
import { createTitlebar } from './parts/titlebar/titlebar';
import { createSidebar } from './parts/sidebar/sidebar';
import { createComputersPage } from './features/computers/computers';
export function renderWorkbench(info: AppInfo, api: DesktopApi): void {
	document.documentElement.dataset.platform = info.platform;
	const status = document.createElement('footer'); status.className = 'status'; status.setAttribute('role', 'status');
	status.textContent = `Desktop preview · ${info.version}`;
	function showError(error: unknown): void {
		console.error(error);
		status.textContent = 'The desktop could not complete the action. Close the window and try again.';
	}
	const workspace = document.createElement('div'); workspace.className = 'workspace';
	const page = createComputersPage(); page.append(status);
	workspace.append(createSidebar(), page);
	document.body.replaceChildren(createTitlebar(info, api, showError), workspace);
}

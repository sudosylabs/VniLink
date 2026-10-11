import { contextBridge, ipcRenderer } from 'electron';
import { isAppInfo } from './platform/ipc/common/desktopApi';
import type { DesktopApi } from './platform/ipc/common/desktopApi';
if (!process.sandboxed || !process.contextIsolated) { throw new Error('Renderer isolation required'); }
const api: DesktopApi = {
	getAppInfo: async () => {
		const info: unknown = await ipcRenderer.invoke('app:info');
		if (!isAppInfo(info)) {
			throw new Error('Invalid application information');
		}
		return info;
	},
	windowAction: async (action) => { await ipcRenderer.invoke('window:action', action); },
};
contextBridge.exposeInMainWorld('vnilink', api);

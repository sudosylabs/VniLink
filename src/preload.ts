import { isEngineCommand, isEngineReply } from './platform/engine/common/engine.js';
import { contextBridge, ipcRenderer } from 'electron';
import { isAppInfo } from './platform/ipc/common/desktopApi.js';
import type { DesktopApi } from './platform/ipc/common/desktopApi.js';
if (!process.sandboxed || !process.contextIsolated) {
	throw new Error('Renderer isolation required');
}
const api: DesktopApi = {
	engineCommand: async (command) => {
		if (!isEngineCommand(command)) {
			throw new Error('Invalid engine command');
		}
		const reply: unknown = await ipcRenderer.invoke('engine:command', command);
		if (!isEngineReply(reply)) {
			throw new Error('Invalid engine reply');
		}
		return reply;
	},
	getAppInfo: async () => {
		const info: unknown = await ipcRenderer.invoke('app:info');
		if (!isAppInfo(info)) {
			throw new Error('Invalid application information');
		}
		return info;
	},
	windowAction: async (action) => {
		await ipcRenderer.invoke('window:action', action);
	},
};
contextBridge.exposeInMainWorld('vnilink', api);

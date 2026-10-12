import { isNativeHostRequest, isWindowState } from './platform/native/common/native.js';
import { isEngineCommand, isEngineReply } from './platform/engine/common/engine.js';
import { isEncryptionText } from './platform/encryption/common/encryption.js';
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
	nativeHostCommand: async (method, args) => {
		if (!isNativeHostRequest(method, args)) {
			throw new Error('Invalid native host request');
		}
		const reply: unknown = await ipcRenderer.invoke('nativeHost:command', method, args);
		if (method === 'getWindowState' && !isWindowState(reply)) {
			throw new Error('Invalid window state');
		}
		return reply;
	},
	encryptText: async (value) => {
		if (!isEncryptionText(value)) {
			throw new Error('Invalid secret value');
		}
		const encrypted: unknown = await ipcRenderer.invoke('encryption:encrypt', value);
		if (!isEncryptionText(encrypted)) {
			throw new Error('Invalid encrypted value');
		}
		return encrypted;
	},
	decryptText: async (value) => {
		if (!isEncryptionText(value)) {
			throw new Error('Invalid encrypted value');
		}
		const decrypted: unknown = await ipcRenderer.invoke('encryption:decrypt', value);
		if (typeof decrypted !== 'string') {
			throw new Error('Invalid secret value');
		}
		return decrypted;
	},
	isEncryptionAvailable: async () => {
		const available: unknown = await ipcRenderer.invoke('encryption:available');
		if (typeof available !== 'boolean') {
			throw new Error('Invalid encryption state');
		}
		return available;
	},
	getKeyStorageProvider: async () => {
		const provider: unknown = await ipcRenderer.invoke('encryption:provider');
		if (typeof provider !== 'string') {
			throw new Error('Invalid storage provider');
		}
		return provider;
	},
	onWindowStateChanged: (listener) => {
		const handler = (_event: Electron.IpcRendererEvent, state: unknown): void => {
			if (isWindowState(state)) {
				listener(state);
			}
		};
		ipcRenderer.on('nativeHost:state', handler);
		return () => {
			ipcRenderer.removeListener('nativeHost:state', handler);
		};
	},
};
contextBridge.exposeInMainWorld('vnilink', api);

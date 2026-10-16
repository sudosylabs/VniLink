import { isRestartState } from './platform/application/common/application.js';
import { isPermissionCommand, isPermissionSnapshot } from './platform/permissions/common/permissions.js';
import { isChannelRequest } from './platform/engine/common/dataChannel.js';
import { isNativeHostRequest, isWindowState } from './platform/native/common/native.js';
import { isEngineCommand, isEngineReply, isNativeScreenSources } from './platform/engine/common/engine.js';
import { isEncryptionText } from './platform/encryption/common/encryption.js';
import { contextBridge, ipcRenderer } from 'electron';
import { isAppInfo } from './platform/ipc/common/desktopApi.js';
import type { DesktopApi } from './platform/ipc/common/desktopApi.js';
import { isConfigurationSnapshot, isConfigurationUpdate } from './platform/configuration/common/configuration.js';

if (!process.sandboxed || !process.contextIsolated) {
	throw new Error('Renderer isolation required');
}

const channelRequests = new Set<string>();

ipcRenderer.on('engine:data-port', (event, requestId: unknown) => {
	if (typeof requestId !== 'string' || !channelRequests.delete(requestId)) {
		for (const port of event.ports) {
			port.close();
		}
		return;
	}
	window.postMessage({ type: 'vnilink:data-port', requestId }, location.origin, event.ports);
});

const api: DesktopApi = {
	recordingDirectory: async action => {
		if (!['read', 'choose', 'destination'].includes(action)) {
			throw new Error('Invalid recording directory action');
		}
		const value: unknown = await ipcRenderer.invoke('app:recording-directory', action);
		if (value !== null && typeof value !== 'string') {
			throw new Error('Invalid recording directory');
		}
		return value;
	},
	getCaptureDisplays: async () => {
		const sources: unknown = await ipcRenderer.invoke('app:capture-displays');
		if (!isNativeScreenSources(sources)) {
			throw new Error('Invalid capture display reply');
		}
		return sources;
	},
	getRestartState: async () => {
		const state: unknown = await ipcRenderer.invoke('app:restart-state');
		if (!isRestartState(state)) {
			throw new Error('Invalid restart state');
		}
		return state;
	},
	onRestartStateChanged: listener => {
		const handler = (_event: Electron.IpcRendererEvent, state: unknown): void => {
			if (isRestartState(state)) {
				listener(state);
			}
		};
		ipcRenderer.on('app:restart-state', handler);
		return () => {
			ipcRenderer.removeListener('app:restart-state', handler);
		};
	},
	restartApplication: async () => {
		await ipcRenderer.invoke('app:restart');
	},
	openRelayDocumentation: async () => {
		await ipcRenderer.invoke('app:relay-documentation');
	},
	permissionCommand: async command => {
		if (!isPermissionCommand(command)) {
			throw new Error('Invalid permission command');
		}
		const snapshot: unknown = await ipcRenderer.invoke('permissions:command', command);
		if (!isPermissionSnapshot(snapshot)) {
			throw new Error('Invalid permission snapshot');
		}
		return snapshot;
	},
	getConfiguration: async () => {
		const snapshot: unknown = await ipcRenderer.invoke('configuration:get');
		if (!isConfigurationSnapshot(snapshot)) {
			throw new Error('Invalid configuration');
		}
		return snapshot;
	},
	updateConfiguration: async change => {
		if (!isConfigurationUpdate(change)) {
			throw new Error('Invalid configuration update');
		}
		const snapshot: unknown = await ipcRenderer.invoke('configuration:update', change);
		if (!isConfigurationSnapshot(snapshot)) {
			throw new Error('Invalid configuration');
		}
		return snapshot;
	},
	onConfigurationChanged: listener => {
		const handler = (_event: Electron.IpcRendererEvent, snapshot: unknown): void => {
			if (isConfigurationSnapshot(snapshot)) {
				listener(snapshot);
			}
		};
		ipcRenderer.on('configuration:changed', handler);
		return () => {
			ipcRenderer.removeListener('configuration:changed', handler);
		};
	},
	openDataChannel: async request => {
		if (!isChannelRequest(request) || channelRequests.size >= 8 || channelRequests.has(request.requestId)) {
			throw new Error('Invalid data channel request');
		}
		channelRequests.add(request.requestId);
		try {
			await ipcRenderer.invoke('engine:open-data-channel', request);
		} catch (error) {
			channelRequests.delete(request.requestId);
			throw error;
		}
	},
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

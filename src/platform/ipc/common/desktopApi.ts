import type { RestartState } from '../../application/common/application.js';
import type { PermissionCommand, PermissionSnapshot } from '../../permissions/common/permissions.js';
import type { DataChannelRequest } from '../../engine/common/dataChannel.js';
import type { NativeHostMethod, IWindowState } from '../../native/common/native.js';
import type { EngineCommand, EngineReply, NativeScreenSource } from '../../engine/common/engine.js';
import type { ConfigurationSnapshot, ConfigurationUpdate } from '../../configuration/common/configuration.js';

export interface AppInfo {
	windowId: number;
	osRelease: string;
	platform: 'darwin' | 'win32' | 'linux';
	version: string;
	engineConnected: boolean;
}

export interface DesktopApi {
	getCaptureDisplays(): Promise<readonly NativeScreenSource[]>;
	getRestartState(): Promise<RestartState>;
	onRestartStateChanged(listener: (state: RestartState) => void): () => void;
	restartApplication(): Promise<void>;
	openRelayDocumentation(): Promise<void>;
	permissionCommand(command: PermissionCommand): Promise<PermissionSnapshot>;
	getConfiguration(): Promise<ConfigurationSnapshot>;
	updateConfiguration(change: ConfigurationUpdate): Promise<ConfigurationSnapshot>;
	onConfigurationChanged(listener: (snapshot: ConfigurationSnapshot) => void): () => void;
	openDataChannel(request: DataChannelRequest): Promise<void>;
	getAppInfo(): Promise<AppInfo>;
	nativeHostCommand(method: NativeHostMethod, args: unknown[]): Promise<unknown>;
	onWindowStateChanged(listener: (state: IWindowState) => void): () => void;
	engineCommand(command: EngineCommand): Promise<EngineReply>;
	encryptText(value: string): Promise<string>;
	decryptText(value: string): Promise<string>;
	isEncryptionAvailable(): Promise<boolean>;
	getKeyStorageProvider(): Promise<string>;
}

export function isAppInfo(value: unknown): value is AppInfo {
	if (typeof value !== 'object' || value === null) {
		return false;
	}
	return (
		'windowId' in value &&
		typeof value.windowId === 'number' &&
		Number.isSafeInteger(value.windowId) &&
		value.windowId > 0 &&
		'osRelease' in value &&
		typeof value.osRelease === 'string' &&
		'platform' in value &&
		(value.platform === 'darwin' || value.platform === 'win32' || value.platform === 'linux') &&
		'version' in value &&
		typeof value.version === 'string' &&
		'engineConnected' in value &&
		typeof value.engineConnected === 'boolean'
	);
}

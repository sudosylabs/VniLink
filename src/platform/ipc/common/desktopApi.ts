import type { EngineCommand, EngineReply } from '../../engine/common/engine.js';
export interface AppInfo {
	platform: 'darwin' | 'win32' | 'linux';
	version: string;
	engineConnected: boolean;
}
export type WindowAction = 'minimize' | 'maximize' | 'close';
export interface DesktopApi {
	getAppInfo(): Promise<AppInfo>;
	windowAction(action: WindowAction): Promise<void>;
	engineCommand(command: EngineCommand): Promise<EngineReply>;
}

export function isAppInfo(value: unknown): value is AppInfo {
	if (typeof value !== 'object' || value === null) {
		return false;
	}
	return 'platform' in value && (value.platform === 'darwin' || value.platform === 'win32' || value.platform === 'linux')
		&& 'version' in value && typeof value.version === 'string'
		&& 'engineConnected' in value && typeof value.engineConnected === 'boolean';
}

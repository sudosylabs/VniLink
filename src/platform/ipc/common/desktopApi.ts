export interface AppInfo {
	platform: 'darwin' | 'win32' | 'linux';
	version: string;
	engineConnected: false;
}
export type WindowAction = 'minimize' | 'maximize' | 'close';
export interface DesktopApi {
	getAppInfo(): Promise<AppInfo>;
	windowAction(action: WindowAction): Promise<void>;
}

export function isAppInfo(value: unknown): value is AppInfo {
	if (typeof value !== 'object' || value === null) {
		return false;
	}
	return 'platform' in value && (value.platform === 'darwin' || value.platform === 'win32' || value.platform === 'linux')
		&& 'version' in value && typeof value.version === 'string'
		&& 'engineConnected' in value && value.engineConnected === false;
}

/** Per-window values the main process hands to the sandboxed workbench before it starts. */
export interface INativeWindowConfiguration {
	readonly windowId: number;
	readonly osRelease: string;
	readonly platform: 'darwin' | 'win32' | 'linux';
	readonly version: string;
	readonly engineAvailable: boolean;
	/** Exposes the automation driver; set only for isolated smoke-test profiles. */
	readonly enableSmokeTestDriver: boolean;
}

/** Channel the preload uses to resolve the window configuration. */
export const WINDOW_CONFIGURATION_CHANNEL = 'vscode:windowConfiguration';

export function isNativeWindowConfiguration(value: unknown): value is INativeWindowConfiguration {
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
		'engineAvailable' in value &&
		typeof value.engineAvailable === 'boolean' &&
		'enableSmokeTestDriver' in value &&
		typeof value.enableSmokeTestDriver === 'boolean'
	);
}

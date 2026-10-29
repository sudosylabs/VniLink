export const UI_STATE_CHANNEL_NAME = 'uiState';

/** Which of a Files tab's two panes: this computer, or the paired one. */
export type FilesPane = 'local' | 'remote';

/** Which corner of a remote desktop the picture's details sit in. */
export type DesktopStatsCorner = 'topLeft' | 'topRight' | 'bottomLeft' | 'bottomRight';

export const desktopStatsCorners: readonly DesktopStatsCorner[] = ['topLeft', 'topRight', 'bottomLeft', 'bottomRight'];

/** A folder as the engine names it: a root id and a path relative to that root. */
export interface FolderLocation {
	readonly root: string;
	readonly path: string;
}

/**
 * What windows may remember between runs, by key. Each key family names the
 * shape of its value, so a window can neither store arbitrary data in the
 * main process's state file nor touch the main process's own keys.
 */
export interface UIStateValues {
	/** The folder last open in `<pane>` of the Files tab for `<deviceId>`: `files.lastFolder.<deviceId>.<pane>`. */
	readonly [key: `files.lastFolder.${string}.${FilesPane}`]: FolderLocation;
	/** Whether the Files tab lists hidden files. */
	readonly 'files.showHidden': boolean;
	/** Where the picture's details sit on the remote desktop of `<deviceId>`: `desktop.statsCorner.<deviceId>`. */
	readonly [key: `desktop.statsCorner.${string}`]: DesktopStatsCorner;
	/** Whether remote desktops show the picture's details over the picture. */
	readonly 'desktop.showStats': boolean;
}

export type UIStateKey = keyof UIStateValues;

export function filesLastFolderKey(deviceId: string, pane: FilesPane): UIStateKey {
	return `files.lastFolder.${deviceId}.${pane}`;
}

export function desktopStatsCornerKey(deviceId: string): `desktop.statsCorner.${string}` {
	return `desktop.statsCorner.${deviceId}`;
}

const lastFolderKey = /^files\.lastFolder\.[A-Za-z0-9_-]{1,128}\.(local|remote)$/;

const statsCornerKey = /^desktop\.statsCorner\.[A-Za-z0-9_-]{1,128}$/;

function record(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function boundedString(value: unknown, maximum: number): value is string {
	return typeof value === 'string' && value.length <= maximum;
}

export function isUIStateKey(value: unknown): value is UIStateKey {
	return typeof value === 'string' && (value === 'files.showHidden' || value === 'desktop.showStats' || lastFolderKey.test(value) || statsCornerKey.test(value));
}

/** Whether `value` may be stored under `key`; `undefined` is never a value, it removes the key. */
export function isUIStateValue<K extends UIStateKey>(key: K, value: unknown): value is UIStateValues[K] {
	if (key === 'files.showHidden' || key === 'desktop.showStats') {
		return typeof value === 'boolean';
	}
	if (statsCornerKey.test(key)) {
		return desktopStatsCorners.includes(value as DesktopStatsCorner);
	}
	return record(value) && Object.keys(value).length === 2 && boundedString(value.root, 256) && value.root.length > 0 && boundedString(value.path, 4096);
}

/** A read names one key; a write also carries a value valid for that key. */
export function isUIStateRead(value: unknown): value is { readonly key: UIStateKey } {
	return record(value) && Object.keys(value).length === 1 && isUIStateKey(value.key);
}

export function isUIStateWrite(value: unknown): value is { readonly key: UIStateKey; readonly value: UIStateValues[UIStateKey] } {
	return record(value) && Object.keys(value).length === 2 && isUIStateKey(value.key) && isUIStateValue(value.key, value.value);
}

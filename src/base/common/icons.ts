/** Product icon IDs. Components depend on these names, not an asset library. */
export const Icons = Object.freeze({
	home: 'home',
	computers: 'computers',
	devices: 'devices',
	apple: 'apple',
	windows: 'windows',
	linux: 'linux',
	remoteDesktop: 'remoteDesktop',
	refresh: 'refresh',
	restart: 'restart',
	back: 'back',
	forward: 'forward',
	chevronLeft: 'chevronLeft',
	chevronRight: 'chevronRight',
	chevronUp: 'chevronUp',
	chevronDown: 'chevronDown',
	settings: 'settings',
	appearance: 'appearance',
	connection: 'connection',
	security: 'security',
	keyboard: 'keyboard',
	files: 'files',
	terminal: 'terminal',
	audio: 'audio',
	recording: 'recording',
	power: 'power',
	importInvitation: 'importInvitation',
	createInvitation: 'createInvitation',
	view: 'view',
	camera: 'camera',
	lock: 'lock',
	copy: 'copy',
	fullScreen: 'fullScreen',
	warning: 'warning',
	info: 'info',
	error: 'error',
	exitFullScreen: 'exitFullScreen',
	fillView: 'fillView',
	fitToView: 'fitToView',
	stop: 'stop',
	check: 'check',

	close: 'close',
	minimize: 'minimize',
	maximize: 'maximize',
	restore: 'restore',
} as const);

export type IconId = typeof Icons[keyof typeof Icons];

export type IconVariant = 'outlined' | 'filled';

const iconIds: ReadonlySet<string> = new Set(Object.values(Icons));

/** Whether `value` names a product icon, such as a `ThemeIcon` id given to a menu item. */
export function isIconId(value: string): value is IconId {
	return iconIds.has(value);
}

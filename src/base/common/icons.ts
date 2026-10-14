/** Product icon IDs. Components depend on these names, not an asset library. */
export const Icons = Object.freeze({
	home: 'home',
	computers: 'computers',
	devices: 'devices',
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

	close: 'close',
	minimize: 'minimize',
	maximize: 'maximize',
	restore: 'restore',
} as const);

export type IconId = typeof Icons[keyof typeof Icons];

export type IconVariant = 'outlined' | 'filled';

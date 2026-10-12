/** Product icon IDs. Components depend on these names, not an asset library. */
export const Icons = Object.freeze({
	home: 'home',
	computers: 'computers',
	refresh: 'refresh',
	back: 'back',
	forward: 'forward',
	chevronLeft: 'chevronLeft',
	chevronRight: 'chevronRight',
	chevronUp: 'chevronUp',
	chevronDown: 'chevronDown',
	settings: 'settings',
	close: 'close',
	minimize: 'minimize',
	maximize: 'maximize',
	restore: 'restore',
} as const);
export type IconId = typeof Icons[keyof typeof Icons];
export type IconVariant = 'outlined' | 'filled';

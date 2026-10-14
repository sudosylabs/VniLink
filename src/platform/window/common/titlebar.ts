import { getNativeTitlebarHeight } from './window.js';

/** Room above the fixed-height view tabs, shared with the native window controls. */
export function getWorkbenchTitlebarHeight(osVersion: string): number {
	return Math.max(40, getNativeTitlebarHeight(osVersion));
}

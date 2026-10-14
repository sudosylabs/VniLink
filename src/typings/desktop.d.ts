import type { DesktopApi } from '../platform/ipc/common/desktopApi.js';

declare global {
	interface Window { vnilink: DesktopApi; }
}

import type { DesktopApi } from '../platform/ipc/common/desktopApi';
declare global { interface Window { vnilink: DesktopApi; } }

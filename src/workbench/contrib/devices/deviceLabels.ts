import * as nls from '../../../nls.js';
import { Icons, type IconId } from '../../../base/common/icons.js';
import type { DeviceSummary } from '../../services/devices/deviceService.js';

export function devicePlatform(device: DeviceSummary): string {
	return device.os === 'macos' ? 'macOS' : device.os === 'windows' ? 'Windows' : device.os === 'linux' ? 'Linux' : nls.localize('devices.computer', 'Computer');
}

export function devicePresence(device: DeviceSummary): string {
	const labels = {
		connected: nls.localize('devices.connected', 'Connected'),
		connecting: nls.localize('devices.connecting', 'Connecting'),
		reconnecting: nls.localize('devices.reconnecting', 'Reconnecting'),
		notAccepting: nls.localize('devices.notAccepting', 'Not accepting connections'),
		disconnected: nls.localize('devices.offline', 'Offline'),
		unknown: nls.localize('devices.unknown', 'Status unavailable'),
	};
	return labels[device.presence];
}

export function platformIcon(device: DeviceSummary): IconId {
	switch (device.os) {
		case 'macos': return Icons.apple;
		case 'windows': return Icons.windows;
		case 'linux': return Icons.linux;
		case 'other':
		case null:
			return Icons.remoteDesktop;
	}
}

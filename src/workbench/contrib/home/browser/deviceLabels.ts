import * as nls from '../../../../nls.js';
import type { DeviceSummary } from '../../../services/devices/common/deviceService.js';

export function devicePlatform(device: DeviceSummary): string {
	return device.os === 'macos' ? 'macOS' : device.os === 'windows' ? 'Windows' : device.os === 'linux' ? 'Linux' : nls.localize('devices.computer', 'Computer');
}

export function devicePresence(device: DeviceSummary): string {
	const labels = {
		connected: nls.localize('devices.connected', 'Connected'),
		connecting: nls.localize('devices.connecting', 'Connecting'),
		reconnecting: nls.localize('devices.reconnecting', 'Reconnecting'),
		disconnected: nls.localize('devices.disconnected', 'Disconnected'),
		unknown: nls.localize('devices.unknown', 'Status unavailable'),
	};
	return labels[device.presence];
}

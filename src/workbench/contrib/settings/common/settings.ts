import * as nls from '../../../../nls.js';
import { Icons, type IconId } from '../../../../base/common/icons.js';

export interface SettingsCategory {
	readonly id: string;
	readonly label: string;
	readonly icon: IconId;
	readonly description: string;
}

/** Navigation metadata only; a category does not imply a working settings backend. */
export const settingsCategories = [
	{ id: 'appearance', label: nls.localize('home.appearance', 'Appearance'), icon: Icons.appearance, description: nls.localize('home.appearanceDescription', 'Theme and window background.') },
	{ id: 'connection', label: nls.localize('home.connection', 'Connection'), icon: Icons.connection, description: nls.localize('home.connectionDescription', 'Relays and how your devices connect.') },
	{ id: 'access', label: nls.localize('home.access', 'Access & Security'), icon: Icons.security, description: nls.localize('home.accessDescription', 'Who can connect and what they can access.') },
	{ id: 'desktop', label: nls.localize('home.desktop', 'Remote Desktop'), icon: Icons.remoteDesktop, description: nls.localize('home.desktopDescription', 'Displays, resolution and streaming quality.') },
	{ id: 'input', label: nls.localize('home.input', 'Keyboard & Mouse'), icon: Icons.keyboard, description: nls.localize('home.inputDescription', 'Keyboard mapping and pointer behavior.') },
	{ id: 'files', label: nls.localize('home.files', 'Files'), icon: Icons.files, description: nls.localize('home.filesDescription', 'Shared folders and file transfers.') },
	{ id: 'terminal', label: nls.localize('home.terminal', 'Terminal'), icon: Icons.terminal, description: nls.localize('home.terminalDescription', 'Remote shells and terminal sessions.') },
	{ id: 'media', label: nls.localize('home.media', 'Audio & Camera'), icon: Icons.audio, description: nls.localize('home.mediaDescription', 'Audio devices, microphones and cameras.') },
	{ id: 'recording', label: nls.localize('home.recording', 'Recording'), icon: Icons.recording, description: nls.localize('home.recordingDescription', 'Session recordings and storage limits.') },
	{ id: 'power', label: nls.localize('home.power', 'Power'), icon: Icons.power, description: nls.localize('home.powerDescription', 'Sleep, idle behavior and locking after sessions.') },
] as const satisfies readonly SettingsCategory[];

export type SettingsCategoryId = typeof settingsCategories[number]['id'];

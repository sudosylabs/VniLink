import * as nls from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import type { SidebarGroup } from './sidebar.js';
import { settingsCategories, type SettingsCategoryId } from '../settings/settingsCategories.js';

export type HomeSectionId = 'devices' | SettingsCategoryId;

const implementedCategories = settingsCategories.filter(category => category.id !== 'files');

export const homeNavigationGroups: readonly SidebarGroup[] = [
	{ items: [{ id: 'devices', label: nls.localize('home.devices', 'Devices'), icon: Icons.devices }] },
	{ label: nls.localize('home.settings', 'Settings'), items: implementedCategories },
];

export function isHomeSectionId(id: string): id is HomeSectionId {
	return id === 'devices' || implementedCategories.some(category => category.id === id);
}

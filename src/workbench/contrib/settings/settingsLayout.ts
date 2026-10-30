// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IconId } from '../../../base/common/icons.js';
import type { IDisposable } from '../../../base/common/lifecycle.js';
import type { SyncDescriptor } from '../../../platform/instantiation/common/descriptors.js';

/** Page content that is not a setting, such as live system state. */
export interface ISettingsWidget extends IDisposable {
	render(container: HTMLElement): void;
	/** Reads its state again while the page is visible. */
	refresh?(): Promise<void>;
}

export interface ISettingsSectionDescriptor {
	readonly id: string;
	readonly title?: string;
	readonly description?: string;
	/** Settings shown in this section, in order. */
	readonly settings?: readonly string[];
	readonly widget?: SyncDescriptor<ISettingsWidget>;
	/** Renders the settings or widget directly in the page, without a section of its own. */
	readonly bare?: boolean;
	readonly className?: string;
}

/** When a page reads its settings and widgets again, for changes made outside this window. */
export const enum SettingsRefresh {
	/** When the page is shown or the window gains focus. */
	Focus = 'focus',
	/** Also every few seconds, for operating-system state with no change event. */
	Interval = 'interval',
}

/**
 * One settings page in Home, in the shape of VS Code's settings layout. The
 * class names keep each page's established look; the page renders the rest.
 */
export interface ISettingsPageDescriptor {
	readonly id: string;
	readonly title: string;
	readonly description: string;
	readonly icon: IconId;
	/** The page element's class; `<id>-page` when omitted. */
	readonly className?: string;
	readonly contentClass: string;
	/** Wraps the sections; omitted when sections sit directly in the content. */
	readonly fieldsClass?: string;
	readonly descriptionClass: string;
	/** The page's status line. */
	readonly statusClass?: string;
	/** A closing remark about when the page's settings take effect. */
	readonly note?: string;
	/** Reads settings and widgets again while the page is visible; omitted, only once when created. */
	readonly refresh?: SettingsRefresh;
	readonly header?: readonly ISettingsSectionDescriptor[];
	readonly sections: readonly ISettingsSectionDescriptor[];
	/** Shown after the status line. */
	readonly footer?: readonly ISettingsSectionDescriptor[];
}

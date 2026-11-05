// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Event } from '../../../base/common/event.js';
import type { IDisposable } from '../../../base/common/lifecycle.js';
import type { IconId } from '../../../base/common/icons.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

/**
 * A tab owns its view or session; selecting another tab never disposes it.
 * Its toolbar actions are menu items under `ViewActionsMenu`, chosen with the
 * `activeTab` context key.
 */
export interface WorkbenchTab extends IDisposable {
	readonly id: string;
	readonly label: string;
	readonly icon?: IconId;
	readonly element: HTMLElement;
	/** Permanent navigation tabs stay first and cannot be closed or reordered. */
	readonly permanent?: boolean;
	/** What the view is drawn on; `opaque` when unset. */
	readonly surface?: ViewSurface;
	canClose?(): boolean | Promise<boolean>;
}

/**
 * A view's own opaque page, or the window's native material, as Windows 11
 * draws its first-run setup on Mica. Only Mica honours `material`: Liquid
 * Glass and vibrancy belong to navigation, so there the view is a page, as it
 * is wherever the window has no material.
 */
export type ViewSurface = 'opaque' | 'material';

export const ITabService = createDecorator<ITabService>('tabService');

export interface ITabService {
	readonly _serviceBrand: undefined;
	readonly tabs: readonly WorkbenchTab[];
	readonly activeTab: WorkbenchTab | undefined;
	readonly onDidChange: Event<void>;
	open(tab: WorkbenchTab): void;
	select(id: string): void;
	move(id: string, index: number): void;
	close(id: string): Promise<boolean>;
}

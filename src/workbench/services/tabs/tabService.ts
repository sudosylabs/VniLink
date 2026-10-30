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
	canClose?(): boolean | Promise<boolean>;
}

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

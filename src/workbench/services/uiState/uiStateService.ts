// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { UIStateKey, UIStateValues } from '../../../platform/state/common/uiState.js';

export const IUIStateService = createDecorator<IUIStateService>('uiStateService');

/**
 * What the window remembers between runs, such as the Files tab's last folder
 * on each computer and whether it shows hidden files. The main process keeps
 * it in the state file in the app's data folder; keys and their value shapes
 * are `UIStateValues`.
 */
export interface IUIStateService {
	readonly _serviceBrand: undefined;
	/** The saved value, or `undefined` when none is saved or the saved one is no longer valid. */
	get<K extends UIStateKey>(key: K): Promise<UIStateValues[K] | undefined>;
	/** Saves `value`; `undefined` forgets the key. */
	set<K extends UIStateKey>(key: K, value: UIStateValues[K] | undefined): Promise<void>;
}

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { TerminalShells } from '../../../platform/engine/common/engine.js';

export const ITerminalHostService = createDecorator<ITerminalHostService>('terminalHostService');

/** What this computer offers paired computers' terminals. */
export interface ITerminalHostService {
	readonly _serviceBrand: undefined;
	/** The shells installed here, what automatic resolves to, and the one offered now. */
	shells(): Promise<TerminalShells>;
}

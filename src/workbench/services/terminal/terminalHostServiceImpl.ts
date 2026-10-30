// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { IEngineService } from '../../../platform/engine/common/engineService.js';
import type { TerminalShells } from '../../../platform/engine/common/engine.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { ITerminalHostService } from './terminalHostService.js';

export class TerminalHostService implements ITerminalHostService {
	declare public readonly _serviceBrand: undefined;

	constructor(@IEngineService private readonly engine: IEngineService) { }

	public async shells(): Promise<TerminalShells> {
		const reply = await this.engine.execute({ type: 'terminalShells' });
		if (reply.type !== 'terminalShells') {
			throw new Error('Unexpected terminal shells reply');
		}
		return { shells: reply.shells, automatic: reply.automatic, current: reply.current };
	}
}

registerSingleton(ITerminalHostService, TerminalHostService, InstantiationType.Delayed);

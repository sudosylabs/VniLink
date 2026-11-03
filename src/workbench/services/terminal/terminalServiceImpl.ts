// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import type { ITerminalChildProcess } from '../../../platform/terminal/common/terminal.js';
import { EnginePty } from './enginePty.js';
import { ITerminalService } from './terminalService.js';

class TerminalService implements ITerminalService {
	declare public readonly _serviceBrand: undefined;

	constructor(
		@IInstantiationService private readonly instantiationService: IInstantiationService,
	) { }

	public createProcess(deviceId: string, cols: number, rows: number): ITerminalChildProcess {
		return this.instantiationService.createInstance(EnginePty, deviceId, cols, rows);
	}
}

registerSingleton(ITerminalService, TerminalService, InstantiationType.Delayed);

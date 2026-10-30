// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { CommandsRegistry, ICommandService, type ICommandEvent } from '../../../platform/commands/common/commands.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IInstantiationService, type ServicesAccessor } from '../../../platform/instantiation/common/instantiation.js';
import { ILogService } from '../../../platform/log/common/log.js';

/**
 * Runs commands from `CommandsRegistry` with the workbench's services, as
 * VS Code's `CommandService` does once extensions are ready. VniLink has no
 * extension host, so commands never wait for activation.
 */
export class CommandService extends Disposable implements ICommandService {
	declare public readonly _serviceBrand: undefined;
	private readonly willExecute = this._register(new Emitter<ICommandEvent>());
	public readonly onWillExecuteCommand = this.willExecute.event;
	private readonly didExecute = this._register(new Emitter<ICommandEvent>());
	public readonly onDidExecuteCommand = this.didExecute.event;

	constructor(
		@IInstantiationService private readonly instantiationService: IInstantiationService,
		@ILogService private readonly logService: ILogService,
	) {
		super();
	}

	public async executeCommand<R = unknown>(id: string, ...args: unknown[]): Promise<R | undefined> {
		this.logService.trace('CommandService#executeCommand', id);
		const command = CommandsRegistry.getCommand(id);
		if (!command) {
			throw new Error(`command '${id}' not found`);
		}
		// Handlers are registered as returning void, but actions may return the promise of their work.
		const handler = command.handler as (accessor: ServicesAccessor, ...args: unknown[]) => unknown;
		this.willExecute.fire({ commandId: id, args });
		const result = this.instantiationService.invokeFunction(handler, ...args);
		this.didExecute.fire({ commandId: id, args });
		return await result as R | undefined;
	}
}

registerSingleton(ICommandService, CommandService, InstantiationType.Delayed);

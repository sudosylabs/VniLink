/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { MutableDisposable } from '../../../base/common/lifecycle.js';
import type { ILogger, LogLevel } from './log.js';
import { AbstractMessageLogger, DEFAULT_LOG_LEVEL, log } from './log.js';

interface ILog {
	level: LogLevel;
	message: string;
}

export class BufferLogger extends AbstractMessageLogger {

	declare public readonly _serviceBrand: undefined;
	private buffer: ILog[] = [];
	private _logger: ILogger | undefined = undefined;
	private readonly _logLevelDisposable = this._register(new MutableDisposable());

	constructor(logLevel: LogLevel = DEFAULT_LOG_LEVEL) {
		super();
		this.setLevel(logLevel);
	}

	public set logger(logger: ILogger) {
		this._logger = logger;
		this.setLevel(logger.getLevel());
		this._logLevelDisposable.value = logger.onDidChangeLogLevel(level => {
			this.setLevel(level);
		});
		for (const { level, message } of this.buffer) {
			log(logger, level, message);
		}
		this.buffer = [];
	}

	protected log(level: LogLevel, message: string): void {
		if (this._logger) {
			log(this._logger, level, message);
		} else if (this.getLevel() <= level) {
			this.buffer.push({ level, message });
		}
	}

	public override dispose(): void {
		this._logger?.dispose();
		super.dispose();
	}

	public override flush(): void {
		this._logger?.flush();
	}
}

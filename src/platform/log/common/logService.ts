/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See License.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { Disposable } from '../../../base/common/lifecycle.js';
import type { Event } from '../../../base/common/event.js';
import type { ILogger, ILogService, LogLevel } from './log.js';
import { MultiplexLogger } from './log.js';

export class LogService extends Disposable implements ILogService {

	declare public readonly _serviceBrand: undefined;
	private readonly logger: ILogger;

	constructor(primaryLogger: ILogger, otherLoggers: ILogger[] = []) {
		super();
		this.logger = new MultiplexLogger([primaryLogger, ...otherLoggers]);
		this._register(primaryLogger.onDidChangeLogLevel(level => {
			this.setLevel(level);
		}));
	}

	public get onDidChangeLogLevel(): Event<LogLevel> {
		return this.logger.onDidChangeLogLevel;
	}

	public setLevel(level: LogLevel): void {
		this.logger.setLevel(level);
	}

	public getLevel(): LogLevel {
		return this.logger.getLevel();
	}

	public trace(message: string, ...args: unknown[]): void {
		this.logger.trace(message, ...args);
	}

	public debug(message: string, ...args: unknown[]): void {
		this.logger.debug(message, ...args);
	}

	public info(message: string, ...args: unknown[]): void {
		this.logger.info(message, ...args);
	}

	public warn(message: string, ...args: unknown[]): void {
		this.logger.warn(message, ...args);
	}

	public error(message: string | Error, ...args: unknown[]): void {
		this.logger.error(message, ...args);
	}

	public flush(): void {
		this.logger.flush();
	}
}

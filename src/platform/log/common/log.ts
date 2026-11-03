/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/platform/log/common/log.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { toErrorMessage } from '../../../base/common/errorMessage.js';
import type { Event } from '../../../base/common/event.js';
import { Emitter } from '../../../base/common/event.js';
import { hash } from '../../../base/common/hash.js';
import type { IDisposable } from '../../../base/common/lifecycle.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { ResourceMap } from '../../../base/common/map.js';
import { isWindows } from '../../../base/common/platform.js';
import { joinPath } from '../../../base/common/resources.js';
import type { Mutable } from '../../../base/common/types.js';
import { isNumber, isString } from '../../../base/common/types.js';
import { URI } from '../../../base/common/uri.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { LogLevel } from './logLevel.js';

export const ILogService = createDecorator<ILogService>('logService');

export const ILoggerService = createDecorator<ILoggerService>('loggerService');

function now(): string {
	return new Date().toISOString();
}

export function isLogLevel(thing: unknown): thing is LogLevel {
	return isNumber(thing);
}

export { LogLevel };

export const DEFAULT_LOG_LEVEL: LogLevel = LogLevel.Info;

export interface ILogger extends IDisposable {
	readonly onDidChangeLogLevel: Event<LogLevel>;
	getLevel(): LogLevel;
	setLevel(level: LogLevel): void;

	trace(message: string, ...args: unknown[]): void;
	debug(message: string, ...args: unknown[]): void;
	info(message: string, ...args: unknown[]): void;
	warn(message: string, ...args: unknown[]): void;
	error(message: string | Error, ...args: unknown[]): void;

	/**
	 * An operation to flush the contents. Can be synchronous.
	 */
	flush(): void;
}

function canLog(loggerLevel: LogLevel, messageLevel: LogLevel): boolean {
	return loggerLevel !== LogLevel.Off && loggerLevel <= messageLevel;
}

export function log(logger: ILogger, level: LogLevel, message: string): void {
	switch (level) {
		case LogLevel.Trace: logger.trace(message); break;
		case LogLevel.Debug: logger.debug(message); break;
		case LogLevel.Info: logger.info(message); break;
		case LogLevel.Warning: logger.warn(message); break;
		case LogLevel.Error: logger.error(message); break;
		case LogLevel.Off: /* do nothing */ break;
		default: throw new Error(`Invalid log level ${String(level)}`);
	}
}

type ConsoleMethod = 'debug' | 'error' | 'info' | 'log' | 'warn';

type ConsoleMethodFn = (...args: unknown[]) => void;

function getConsoleMethod(method: ConsoleMethod): ConsoleMethodFn {
	switch (method) {
		case 'debug': return console.debug;
		case 'error': return console.error;
		case 'info': return console.info;
		case 'log': return console.log;
		case 'warn': return console.warn;
	}
}

function logToConsole(method: ConsoleMethod, ...args: unknown[]): void {
	getConsoleMethod(method).apply(console, args);
}

export function format(args: readonly unknown[], verbose = false): string {
	let result = '';
	for (let i = 0; i < args.length; i++) {
		let a = args[i];
		if (a instanceof Error) {
			a = toErrorMessage(a, verbose);
		}
		if (typeof a === 'object') {
			try {
				a = JSON.stringify(a);
			} catch { /* Circular values fall back to their string representation. */ }
		}
		result += (i > 0 ? ' ' : '') + String(a);
	}
	return result;
}

interface LoggerGroup {
	readonly id: string;
	readonly name: string;
}

export interface ILogService extends ILogger {
	readonly _serviceBrand: undefined;
}

export interface ILoggerOptions {

	/**
	 * Id of the logger.
	 */
	id?: string;

	/**
	 * Name of the logger.
	 */
	name?: string;

	/**
	 * Do not create rotating files if max size exceeds.
	 */
	donotRotate?: boolean;

	/**
	 * Do not use formatters.
	 */
	donotUseFormatters?: boolean;

	/**
	 * When to log. Set to `always` to log always.
	 */
	logLevel?: 'always' | LogLevel;

	/**
	 * Whether the log should be hidden from the user.
	 */
	hidden?: boolean;

	/**
	 * Condition which must be true to show this logger
	 */
	when?: string;

	/**
	 * Id of the extension that created this logger.
	 */
	extensionId?: string;

	/**
	 * Group of the logger.
	 */
	group?: LoggerGroup;
}

export interface ILoggerResource {
	readonly resource: URI;
	readonly id: string;
	readonly name?: string;
	readonly logLevel?: LogLevel;
	readonly hidden?: boolean;
	readonly when?: string;
	readonly extensionId?: string;
	readonly group?: LoggerGroup;
}

export interface DidChangeLoggersEvent {
	readonly added: Iterable<ILoggerResource>;
	readonly removed: Iterable<ILoggerResource>;
}

export interface ILoggerService {

	readonly _serviceBrand: undefined;

	/**
	 * Creates a logger for the given resource, or gets one if it already exists.
	 *
	 * This will also register the logger with the logger service.
	 */
	createLogger(resource: URI, options?: ILoggerOptions): ILogger;

	/**
	 * Creates a logger with the given id in the logs folder, or gets one if it already exists.
	 *
	 * This will also register the logger with the logger service.
	 */
	createLogger(id: string, options?: Omit<ILoggerOptions, 'id'>): ILogger;

	/**
	 * An event which fires when the log level of a logger has changed
	 */
	readonly onDidChangeLogLevel: Event<LogLevel | [URI, LogLevel]>;

	/**
	 * Set default log level.
	 */
	setLogLevel(level: LogLevel): void;

	/**
	 * Set log level for a logger.
	 */
	setLogLevel(resource: URI, level: LogLevel): void;

	/**
	 * Get log level for a logger or the default log level.
	 */
	getLogLevel(resource?: URI): LogLevel;

	/**
	 * An event which fires when the visibility of a logger has changed
	 */
	readonly onDidChangeVisibility: Event<[URI, boolean]>;

	/**
	 * Set the visibility of a logger.
	 */
	setVisibility(resourceOrId: URI | string, visible: boolean): void;

	/**
	 * An event which fires when the logger resources are changed
	 */
	readonly onDidChangeLoggers: Event<DidChangeLoggersEvent>;

	/**
	 * Register a logger with the logger service.
	 *
	 * Note that this will not create a logger, but only register it.
	 *
	 * Use `createLogger` to create a logger and register it.
	 *
	 * Use it when you want to register a logger that is not created by the logger service.
	 */
	registerLogger(resource: ILoggerResource): void;

	/**
	 * Deregister the logger for the given resource.
	 */
	deregisterLogger(idOrResource: URI | string): void;
}

abstract class AbstractLogger extends Disposable implements ILogger {

	private level: LogLevel = DEFAULT_LOG_LEVEL;
	private readonly _onDidChangeLogLevel: Emitter<LogLevel> = this._register(new Emitter<LogLevel>());

	public get onDidChangeLogLevel(): Event<LogLevel> {
		return this._onDidChangeLogLevel.event;
	}

	public setLevel(level: LogLevel): void {
		if (this.level !== level) {
			this.level = level;
			this._onDidChangeLogLevel.fire(this.level);
		}
	}

	public getLevel(): LogLevel {
		return this.level;
	}

	protected checkLogLevel(level: LogLevel): boolean {
		return canLog(this.level, level);
	}

	protected canLog(level: LogLevel): boolean {
		if (this._store.isDisposed) {
			return false;
		}
		return this.checkLogLevel(level);
	}

	public abstract trace(message: string, ...args: unknown[]): void;

	public abstract debug(message: string, ...args: unknown[]): void;

	public abstract info(message: string, ...args: unknown[]): void;

	public abstract warn(message: string, ...args: unknown[]): void;

	public abstract error(message: string | Error, ...args: unknown[]): void;

	public abstract flush(): void;
}

export abstract class AbstractMessageLogger extends AbstractLogger implements ILogger {

	constructor(private readonly logAlways?: boolean) {
		super();
	}

	protected override checkLogLevel(level: LogLevel): boolean {
		return this.logAlways === true || super.checkLogLevel(level);
	}

	public trace(message: string, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Trace)) {
			this.log(LogLevel.Trace, format([message, ...args], true));
		}
	}

	public debug(message: string, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Debug)) {
			this.log(LogLevel.Debug, format([message, ...args]));
		}
	}

	public info(message: string, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Info)) {
			this.log(LogLevel.Info, format([message, ...args]));
		}
	}

	public warn(message: string, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Warning)) {
			this.log(LogLevel.Warning, format([message, ...args]));
		}
	}

	public error(message: string | Error, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Error)) {
			if (message instanceof Error) {
				this.log(LogLevel.Error, format([message.stack, ...args]));
			} else {
				this.log(LogLevel.Error, format([message, ...args]));
			}
		}
	}

	public flush(): void { /* No buffered output. */ }

	protected abstract log(level: LogLevel, message: string): void;
}

export class ConsoleMainLogger extends AbstractLogger implements ILogger {

	private useColors: boolean;

	constructor(logLevel: LogLevel = DEFAULT_LOG_LEVEL) {
		super();
		this.setLevel(logLevel);
		this.useColors = !isWindows;
	}

	public trace(message: string, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Trace)) {
			if (this.useColors) {
				logToConsole('log', `\x1b[90m[main ${now()}]\x1b[0m`, message, ...args);
			} else {
				logToConsole('log', `[main ${now()}]`, message, ...args);
			}
		}
	}

	public debug(message: string, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Debug)) {
			if (this.useColors) {
				logToConsole('log', `\x1b[90m[main ${now()}]\x1b[0m`, message, ...args);
			} else {
				logToConsole('log', `[main ${now()}]`, message, ...args);
			}
		}
	}

	public info(message: string, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Info)) {
			if (this.useColors) {
				logToConsole('log', `\x1b[90m[main ${now()}]\x1b[0m`, message, ...args);
			} else {
				logToConsole('log', `[main ${now()}]`, message, ...args);
			}
		}
	}

	public warn(message: string | Error, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Warning)) {
			if (this.useColors) {
				logToConsole('warn', `\x1b[93m[main ${now()}]\x1b[0m`, message, ...args);
			} else {
				logToConsole('warn', `[main ${now()}]`, message, ...args);
			}
		}
	}

	public error(message: string | Error, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Error)) {
			if (this.useColors) {
				logToConsole('error', `\x1b[91m[main ${now()}]\x1b[0m`, message, ...args);
			} else {
				logToConsole('error', `[main ${now()}]`, message, ...args);
			}
		}
	}

	public flush(): void {
		// noop
	}

}

export class ConsoleLogger extends AbstractLogger implements ILogger {

	constructor(logLevel: LogLevel = DEFAULT_LOG_LEVEL, private readonly useColors = true) {
		super();
		this.setLevel(logLevel);
	}

	public trace(message: string, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Trace)) {
			if (this.useColors) {
				logToConsole('log', '%cTRACE', 'color: #888', message, ...args);
			} else {
				logToConsole('log', message, ...args);
			}
		}
	}

	public debug(message: string, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Debug)) {
			if (this.useColors) {
				logToConsole('log', '%cDEBUG', 'background: #eee; color: #888', message, ...args);
			} else {
				logToConsole('log', message, ...args);
			}
		}
	}

	public info(message: string, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Info)) {
			if (this.useColors) {
				logToConsole('log', '%c INFO', 'color: #33f', message, ...args);
			} else {
				logToConsole('log', message, ...args);
			}
		}
	}

	public warn(message: string | Error, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Warning)) {
			if (this.useColors) {
				logToConsole('warn', '%c WARN', 'color: #993', message, ...args);
			} else {
				logToConsole('log', message, ...args);
			}
		}
	}

	public error(message: string | Error, ...args: unknown[]): void {
		if (this.canLog(LogLevel.Error)) {
			if (this.useColors) {
				logToConsole('error', '%c  ERR', 'color: #f33', message, ...args);
			} else {
				logToConsole('error', message, ...args);
			}
		}
	}

	public flush(): void {
		// noop
	}
}

export class MultiplexLogger extends AbstractLogger implements ILogger {

	constructor(private readonly loggers: readonly ILogger[]) {
		super();
		if (loggers.length) {
			this.setLevel(loggers[0].getLevel());
		}
	}

	public override setLevel(level: LogLevel): void {
		for (const logger of this.loggers) {
			logger.setLevel(level);
		}
		super.setLevel(level);
	}

	public trace(message: string, ...args: unknown[]): void {
		for (const logger of this.loggers) {
			logger.trace(message, ...args);
		}
	}

	public debug(message: string, ...args: unknown[]): void {
		for (const logger of this.loggers) {
			logger.debug(message, ...args);
		}
	}

	public info(message: string, ...args: unknown[]): void {
		for (const logger of this.loggers) {
			logger.info(message, ...args);
		}
	}

	public warn(message: string, ...args: unknown[]): void {
		for (const logger of this.loggers) {
			logger.warn(message, ...args);
		}
	}

	public error(message: string | Error, ...args: unknown[]): void {
		for (const logger of this.loggers) {
			logger.error(message, ...args);
		}
	}

	public flush(): void {
		for (const logger of this.loggers) {
			logger.flush();
		}
	}

	public override dispose(): void {
		for (const logger of this.loggers) {
			logger.dispose();
		}
		super.dispose();
	}
}

interface LoggerEntry { logger: ILogger | undefined; info: Mutable<ILoggerResource> }

export abstract class AbstractLoggerService extends Disposable implements ILoggerService {

	declare public readonly _serviceBrand: undefined;
	private readonly _loggers = new ResourceMap<LoggerEntry>();
	private _onDidChangeLoggers = this._register(new Emitter<{ added: ILoggerResource[]; removed: ILoggerResource[] }>);
	public readonly onDidChangeLoggers = this._onDidChangeLoggers.event;
	private _onDidChangeLogLevel = this._register(new Emitter<LogLevel | [URI, LogLevel]>);
	public readonly onDidChangeLogLevel = this._onDidChangeLogLevel.event;
	private _onDidChangeVisibility = this._register(new Emitter<[URI, boolean]>);
	public readonly onDidChangeVisibility = this._onDidChangeVisibility.event;

	constructor(
		protected logLevel: LogLevel,
		private readonly logsHome: URI,
		loggerResources?: Iterable<ILoggerResource>,
	) {
		super();
		if (loggerResources) {
			for (const loggerResource of loggerResources) {
				this._loggers.set(loggerResource.resource, { logger: undefined, info: loggerResource });
			}
		}
	}

	private getLoggerEntry(resourceOrId: URI | string): LoggerEntry | undefined {
		if (isString(resourceOrId)) {
			return [...this._loggers.values()].find(logger => logger.info.id === resourceOrId);
		}
		return this._loggers.get(resourceOrId);
	}

	public createLogger(idOrResource: URI | string, options?: ILoggerOptions): ILogger {
		const resource = this.toResource(idOrResource);
		const id = isString(idOrResource) ? idOrResource : (options?.id ?? hash(resource.toString()).toString(16));
		let logger = this._loggers.get(resource)?.logger;
		const logLevel = options?.logLevel === 'always' ? LogLevel.Trace : options?.logLevel;
		logger ??= this.doCreateLogger(resource, logLevel ?? this.getLogLevel(resource), { ...options, id });
		const loggerEntry: LoggerEntry = {
			logger,
			info: {
				resource,
				id,
				logLevel,
				name: options?.name,
				hidden: options?.hidden,
				group: options?.group,
				extensionId: options?.extensionId,
				when: options?.when
			}
		};
		this.registerLogger(loggerEntry.info);
		// TODO: @sandy081 Remove this once registerLogger can take ILogger
		this._loggers.set(resource, loggerEntry);
		return logger;
	}

	protected toResource(idOrResource: string | URI): URI {
		return isString(idOrResource) ? joinPath(this.logsHome, `${idOrResource.replace(/[\\/:\*\?"<>\|]/g, '')}.log`) : idOrResource;
	}

	public setLogLevel(logLevel: LogLevel): void;

	public setLogLevel(resource: URI, logLevel: LogLevel): void;

	public setLogLevel(arg1: URI | LogLevel, arg2?: LogLevel): void {
		if (URI.isUri(arg1)) {
			if (arg2 === undefined) {
				throw new Error('Missing resource log level');
			}
			const resource = arg1;
			const logLevel = arg2;
			const logger = this._loggers.get(resource);
			if (logger && logLevel !== logger.info.logLevel) {
				logger.info.logLevel = logLevel === this.logLevel ? undefined : logLevel;
				logger.logger?.setLevel(logLevel);
				this._loggers.set(logger.info.resource, logger);
				this._onDidChangeLogLevel.fire([resource, logLevel]);
			}
		} else {
			this.logLevel = arg1;
			for (const [resource, logger] of this._loggers.entries()) {
				if (this._loggers.get(resource)?.info.logLevel === undefined) {
					logger.logger?.setLevel(this.logLevel);
				}
			}
			this._onDidChangeLogLevel.fire(this.logLevel);
		}
	}

	public setVisibility(resourceOrId: URI | string, visibility: boolean): void {
		const logger = this.getLoggerEntry(resourceOrId);
		if (logger && visibility !== !logger.info.hidden) {
			logger.info.hidden = !visibility;
			this._loggers.set(logger.info.resource, logger);
			this._onDidChangeVisibility.fire([logger.info.resource, visibility]);
		}
	}

	public getLogLevel(resource?: URI): LogLevel {
		let logLevel;
		if (resource) {
			logLevel = this._loggers.get(resource)?.info.logLevel;
		}
		return logLevel ?? this.logLevel;
	}

	public registerLogger(resource: ILoggerResource): void {
		const existing = this._loggers.get(resource.resource);
		if (existing) {
			if (existing.info.hidden !== resource.hidden) {
				this.setVisibility(resource.resource, !resource.hidden);
			}
		} else {
			this._loggers.set(resource.resource, { info: resource, logger: undefined });
			this._onDidChangeLoggers.fire({ added: [resource], removed: [] });
		}
	}

	public deregisterLogger(idOrResource: URI | string): void {
		const resource = this.toResource(idOrResource);
		const existing = this._loggers.get(resource);
		if (existing) {
			if (existing.logger) {
				existing.logger.dispose();
			}
			this._loggers.delete(resource);
			this._onDidChangeLoggers.fire({ added: [], removed: [existing.info] });
		}
	}

	public override dispose(): void {
		this._loggers.forEach(logger => logger.logger?.dispose());
		this._loggers.clear();
		super.dispose();
	}

	protected abstract doCreateLogger(resource: URI, logLevel: LogLevel, options?: ILoggerOptions): ILogger;
}

class NullLogger implements ILogger {
	public readonly onDidChangeLogLevel: Event<LogLevel> = new Emitter<LogLevel>().event;

	public setLevel(level: LogLevel): void;

	public setLevel(): void { /* Null logger ignores levels. */ }

	public getLevel(): LogLevel {
		return LogLevel.Info;
	}

	public trace(message: string, ...args: unknown[]): void;

	public trace(): void { /* Null logger discards output. */ }

	public debug(message: string, ...args: unknown[]): void;

	public debug(): void { /* Null logger discards output. */ }

	public info(message: string, ...args: unknown[]): void;

	public info(): void { /* Null logger discards output. */ }

	public warn(message: string, ...args: unknown[]): void;

	public warn(): void { /* Null logger discards output. */ }

	public error(message: string | Error, ...args: unknown[]): void;

	public error(): void { /* Null logger discards output. */ }

	public dispose(): void { /* No resources. */ }

	public flush(): void { /* No buffered output. */ }
}

export class NullLogService extends NullLogger implements ILogService {
	declare public readonly _serviceBrand: undefined;
}

export function parseLogLevel(logLevel: string): LogLevel | undefined {
	switch (logLevel) {
		case 'trace':
			return LogLevel.Trace;
		case 'debug':
			return LogLevel.Debug;
		case 'info':
			return LogLevel.Info;
		case 'warn':
			return LogLevel.Warning;
		case 'error':
			return LogLevel.Error;
		case 'critical':
			return LogLevel.Error;
		case 'off':
			return LogLevel.Off;
	}
	return undefined;
}

// CONTEXT_LOG_LEVEL from the upstream module is not ported yet: context keys
// belong with the contextkey platform, which has no counterpart here.

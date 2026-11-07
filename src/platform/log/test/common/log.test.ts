// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { isDisposable } from '../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { BufferLogger } from '../../common/bufferLog.js';
import { AbstractMessageLogger, format, LogLevel, parseLogLevel } from '../../common/log.js';
import { LogService } from '../../common/logService.js';

/** A logger that keeps every message it writes, with its level. */
class CaptureLogger extends AbstractMessageLogger {
	public readonly messages: { readonly level: LogLevel; readonly message: string }[] = [];

	protected log(level: LogLevel, message: string): void {
		this.messages.push({ level, message });
	}
}

suite('log', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	suite('levels', () => {
		for (const [name, level] of [['trace', LogLevel.Trace], ['critical', LogLevel.Error], ['nope', undefined]] as const) {
			test(`'${name}' parses as ${level === undefined ? 'no level' : LogLevel[level]}`, () => {
				assert.strictEqual(parseLogLevel(name), level);
			});
		}
		test('levels are ordered from trace to error', () => {
			const levels = [LogLevel.Trace, LogLevel.Debug, LogLevel.Info, LogLevel.Warning, LogLevel.Error];
			assert.deepStrictEqual([...levels].sort((a, b) => a - b), levels);
		});
	});
	suite('BufferLogger', () => {
		test('messages logged before the logger arrives reach it in order', () => {
			const buffer = disposables.add(new BufferLogger(LogLevel.Trace));
			buffer.info('first');
			buffer.warn('second');
			const target = disposables.add(new CaptureLogger());
			target.setLevel(LogLevel.Trace);
			buffer.logger = target;
			assert.deepStrictEqual(target.messages, [{ level: LogLevel.Info, message: 'first' }, { level: LogLevel.Warning, message: 'second' }]);
		});
	});
	suite('LogService', () => {
		/**
		 * A service on `primary` and `others`. `LogService`, as upstream, never
		 * disposes the multiplexer it builds, so the test does.
		 */
		function createService(primary: CaptureLogger, others: CaptureLogger[] = []): LogService {
			const service = disposables.add(new LogService(primary, others));
			const multiplexer: unknown = Reflect.get(service, 'logger');
			assert.ok(isDisposable(multiplexer));
			disposables.add(multiplexer);
			return service;
		}
		test('the service starts at its primary logger\'s level', () => {
			const primary = disposables.add(new CaptureLogger());
			primary.setLevel(LogLevel.Error);
			const service = createService(primary);
			assert.strictEqual(service.getLevel(), LogLevel.Error);
		});
		test('a level set on the service applies to its loggers', () => {
			const primary = disposables.add(new CaptureLogger());
			const other = disposables.add(new CaptureLogger());
			const service = createService(primary, [other]);
			service.setLevel(LogLevel.Trace);
			assert.deepStrictEqual([service.getLevel(), primary.getLevel(), other.getLevel()], [LogLevel.Trace, LogLevel.Trace, LogLevel.Trace]);
		});
		test('a message reaches every logger', () => {
			const primary = disposables.add(new CaptureLogger());
			const other = disposables.add(new CaptureLogger());
			const service = createService(primary, [other]);
			service.info('started');
			assert.deepStrictEqual([primary.messages, other.messages], [[{ level: LogLevel.Info, message: 'started' }], [{ level: LogLevel.Info, message: 'started' }]]);
		});
	});
	suite('message format', () => {
		test('a circular value falls back to its string form', () => {
			const circular: Record<string, unknown> = {};
			circular.self = circular;
			assert.strictEqual(format([circular, Symbol('value')]), '[object Object] Symbol(value)');
		});
		test('an error keeps its message and the arguments logged with it', () => {
			const logger = disposables.add(new CaptureLogger());
			logger.setLevel(LogLevel.Info);
			logger.info('visible');
			logger.error(new Error('failure'), { detail: 'context' });
			assert.strictEqual(logger.messages.length, 2);
			assert.match(logger.messages[1].message, /failure/);
			assert.match(logger.messages[1].message, /"detail":"context"/);
		});
		test('a message below the logger\'s level is not written', () => {
			const logger = disposables.add(new CaptureLogger());
			logger.setLevel(LogLevel.Info);
			logger.debug('hidden');
			assert.deepStrictEqual(logger.messages, []);
		});
	});
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { posix } from 'node:path';
import type { Event } from '../../../base/common/event.js';
import { URI } from '../../../base/common/uri.js';
import type { IServerChannel } from '../../../base/parts/ipc/common/ipc.js';
import { windowIdFromContext } from '../../ipc/electron-main/windowContext.js';
import { isKnownLogLevel } from '../common/logLevel.js';

/** At most this many messages a window sends in one `log` call. */
const maxMessages = 1000;

/** The longest message a window logs, in characters. */
const maxMessageLength = 64 * 1024;

function record(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isMessages(value: unknown): value is [number, string][] {
	return Array.isArray(value) && value.length <= maxMessages && value.every((message: unknown) =>
		Array.isArray(message) && message.length === 2 && isKnownLogLevel(message[0]) && typeof message[1] === 'string' && message[1].length <= maxMessageLength);
}

/**
 * The facade over the copied `LoggerChannel`, which trusts its caller. A
 * window may create, write and register loggers only for files in its own
 * `window<id>` folder of this run's logs, and the window id is the one the IPC
 * context names, never one the window sends. Changing the global log level and
 * echoing to main's console are not offered to windows.
 */
export class WindowLoggerChannel implements IServerChannel {

	constructor(
		private readonly inner: IServerChannel,
		private readonly logsHome: URI,
	) { }

	public listen<T>(context: string, event: string): Event<T> {
		if (event !== 'onDidChangeLoggers' && event !== 'onDidChangeLogLevel' && event !== 'onDidChangeVisibility') {
			throw new Error(`Event not found: ${event}`);
		}
		return this.inner.listen<T>(context, event, windowIdFromContext(context));
	}

	public call<T>(context: string, command: string, arg?: unknown): Promise<T> {
		const windowId = windowIdFromContext(context);
		const args = Array.isArray(arg) ? arg : [];
		switch (command) {
			case 'createLogger':
				if (args.length > 3 || !(args[1] === undefined || record(args[1]))) {
					break;
				}
				return this.inner.call(context, command, [this.windowResource(windowId, args[0]), args[1], windowId]);
			case 'log':
				if (args.length !== 2 || !isMessages(args[1])) {
					break;
				}
				return this.inner.call(context, command, [this.windowResource(windowId, args[0]), args[1]]);
			case 'setLogLevel':
				if (args.length !== 2 || !isKnownLogLevel(args[1])) {
					break;
				}
				return this.inner.call(context, command, [this.windowResource(windowId, args[0]), args[1]]);
			case 'setVisibility':
				if (args.length !== 2 || typeof args[1] !== 'boolean') {
					break;
				}
				return this.inner.call(context, command, [this.windowResource(windowId, args[0]), args[1]]);
			case 'registerLogger': {
				const logger: unknown = args[0];
				if (args.length > 2 || !record(logger)) {
					break;
				}
				return this.inner.call(context, command, [{ ...logger, resource: this.windowResource(windowId, logger.resource) }, windowId]);
			}
			case 'deregisterLogger':
				return this.inner.call(context, command, [this.windowResource(windowId, args[0]), windowId]);
		}
		throw new Error(`Invalid logger request: ${command}`);
	}

	/** `value` as a file in the calling window's logs folder; anything else is refused. */
	private windowResource(windowId: number, value: unknown): URI {
		if (!record(value) || value.scheme !== 'file' || typeof value.path !== 'string') {
			throw new Error('A window logs only to files');
		}
		// The window was given `logsHome.fsPath` and builds its URIs from that, so compare in the same form.
		const folder = posix.join(URI.file(this.logsHome.fsPath).path, `window${String(windowId)}`);
		const path = posix.normalize(value.path);
		if (path !== value.path || !path.startsWith(`${folder}/`)) {
			throw new Error('A window logs only into its own logs folder');
		}
		return URI.from({ scheme: 'file', path });
	}
}

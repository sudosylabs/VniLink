// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { Event } from '../../../../base/common/event.js';
import { Disposable, DisposableStore } from '../../../../base/common/lifecycle.js';
import { URI } from '../../../../base/common/uri.js';
import type { IServerChannel } from '../../../../base/parts/ipc/common/ipc.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { WindowLoggerChannel } from '../../electron-main/windowLoggerChannel.js';

interface ChannelCall {
	readonly context: string;
	readonly name: string;
	readonly arg: unknown;
}

/** The logger channel the facade guards: it records every call and listen it is passed. */
class TestServerChannel implements IServerChannel {
	public readonly calls: ChannelCall[] = [];

	public call<T>(context: string, command: string, arg?: unknown): Promise<T> {
		this.calls.push({ context, name: command, arg });
		return Promise.resolve(undefined as T);
	}

	public listen<T>(context: string, event: string, arg?: unknown): Event<T> {
		this.calls.push({ context, name: event, arg });
		return () => Disposable.None;
	}
}

const logsHome = URI.file('/profile/logs/20261102T000000');

/** A log file in window 2's own folder, as the window sends it. */
const own = { scheme: 'file', path: '/profile/logs/20261102T000000/window2/renderer.log' };

suite('WindowLoggerChannel', () => {
	const disposables = new DisposableStore();
	let inner: TestServerChannel;
	let channel: WindowLoggerChannel;
	setup(() => {
		inner = new TestServerChannel();
		channel = new WindowLoggerChannel(inner, logsHome);
	});
	teardown(() => {
		disposables.clear();
	});
	test('a window creates a logger in its own folder, under the id its IPC context names', async () => {
		await channel.call('window:2', 'createLogger', [own, { name: 'Window' }, 7]);
		assert.strictEqual(inner.calls.length, 1);
		const [resource, options, windowId] = inner.calls[0].arg as [URI, unknown, number];
		assert.strictEqual(inner.calls[0].name, 'createLogger');
		assert.strictEqual(resource.path, own.path);
		assert.deepStrictEqual(options, { name: 'Window' });
		assert.strictEqual(windowId, 2, 'the window id the window sent is replaced by its own');
	});
	test('a window writes to its own log file', async () => {
		await channel.call('window:2', 'log', [own, [[3, 'started']]]);
		assert.strictEqual(inner.calls[0].name, 'log');
		const [resource, messages] = inner.calls[0].arg as [URI, unknown];
		assert.strictEqual(resource.path, own.path);
		assert.deepStrictEqual(messages, [[3, 'started']]);
	});
	test('a window registers a logger under the id its IPC context names', async () => {
		await channel.call('window:2', 'registerLogger', [{ id: 'renderer', resource: own }, 7]);
		const [logger, windowId] = inner.calls[0].arg as [{ id: string; resource: URI }, number];
		assert.strictEqual(logger.id, 'renderer');
		assert.strictEqual(logger.resource.path, own.path);
		assert.strictEqual(windowId, 2);
	});
	test('a window listens for logger changes under the id its IPC context names', () => {
		channel.listen('window:2', 'onDidChangeLoggers');
		assert.deepStrictEqual(inner.calls, [{ context: 'window:2', name: 'onDidChangeLoggers', arg: 2 }]);
	});
	const refused: [string, string, string, unknown][] = [
		['a path outside the logs folder', 'window:2', 'createLogger', [{ scheme: 'file', path: '/profile/.bashrc' }, {}]],
		['a path that climbs out of the window\'s folder', 'window:2', 'createLogger', [{ scheme: 'file', path: '/profile/logs/20261102T000000/window2/../../main.log' }, {}]],
		['another window\'s folder', 'window:2', 'createLogger', [{ scheme: 'file', path: '/profile/logs/20261102T000000/window3/renderer.log' }, {}]],
		['a resource that is not a file', 'window:2', 'createLogger', [{ scheme: 'http', path: own.path }, {}]],
		['registering main\'s log', 'window:2', 'registerLogger', [{ id: 'main', resource: { scheme: 'file', path: '/profile/logs/20261102T000000/main.log' } }]],
		['a message at an unknown level', 'window:2', 'log', [own, [[9, 'not a level']]]],
		['a message longer than 64 KiB', 'window:2', 'log', [own, [[3, 'x'.repeat(64 * 1024 + 1)]]]],
		['changing the global log level', 'window:2', 'setLogLevel', [0]],
		['echoing to main\'s console', 'window:2', 'consoleLog', [3, ['echo in main']]],
		['a caller that is not a window', 'main', 'createLogger', [own, {}]],
	];
	for (const [name, context, command, arg] of refused) {
		test(`${name} is refused before main logs anything`, async () => {
			await assert.rejects(async () => channel.call(context, command, arg));
			assert.deepStrictEqual(inner.calls, []);
		});
	}
	test('an event the facade does not offer is refused', () => {
		assert.throws(() => channel.listen('window:2', 'onDidSomethingElse'), /Event not found/);
		assert.deepStrictEqual(inner.calls, []);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

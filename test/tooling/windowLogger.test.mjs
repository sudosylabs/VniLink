// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const bundle = await build({
	stdin: { contents: `export { WindowLoggerChannel } from './src/platform/log/electron-main/windowLoggerChannel.js'; export { URI } from './src/base/common/uri.js';`, loader: 'ts', resolveDir: process.cwd() },
	bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json',
});

const { WindowLoggerChannel, URI } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

const logsHome = URI.file('/profile/logs/20261102T000000');

function channel() {
	const calls = [];
	const inner = {
		call: async (context, command, arg) => void calls.push({ context, command, arg }),
		listen: (context, event, arg) => {
			calls.push({ context, event, arg });
			return () => ({ dispose() { } });
		},
	};
	return { facade: new WindowLoggerChannel(inner, logsHome), calls };
}

const own = { scheme: 'file', path: '/profile/logs/20261102T000000/window2/renderer.log' };

test('a window creates and writes loggers in its own logs folder, under the id its IPC context names', async () => {
	const { facade, calls } = channel();
	await facade.call('window:2', 'createLogger', [own, { name: 'Window' }, 7]);
	await facade.call('window:2', 'log', [own, [[3, 'started']]]);
	await facade.call('window:2', 'registerLogger', [{ id: 'renderer', resource: own }, 7]);
	facade.listen('window:2', 'onDidChangeLoggers');
	assert.deepEqual(calls.map(call => call.command ?? call.event), ['createLogger', 'log', 'registerLogger', 'onDidChangeLoggers']);
	assert.equal(calls[0].arg[2], 2, 'The window id the window sent was used');
	assert.equal(calls[0].arg[0].path, own.path);
	assert.equal(calls[2].arg[1], 2);
	assert.equal(calls[3].arg, 2);
});

test('paths outside the window\'s folder, other windows and non-window callers are refused before main logs anything', async () => {
	const { facade, calls } = channel();
	const refused = [
		['window:2', 'createLogger', [{ scheme: 'file', path: '/profile/.bashrc' }, {}]],
		['window:2', 'createLogger', [{ scheme: 'file', path: '/profile/logs/20261102T000000/window2/../../main.log' }, {}]],
		['window:2', 'createLogger', [{ scheme: 'file', path: '/profile/logs/20261102T000000/window3/renderer.log' }, {}]],
		['window:2', 'createLogger', [{ scheme: 'http', path: own.path }, {}]],
		['window:2', 'registerLogger', [{ id: 'main', resource: { scheme: 'file', path: '/profile/logs/20261102T000000/main.log' } }]],
		['window:2', 'log', [own, [[9, 'not a level']]]],
		['window:2', 'log', [own, [[3, 'x'.repeat(64 * 1024 + 1)]]]],
		['window:2', 'setLogLevel', [0]],
		['window:2', 'consoleLog', [3, ['echo in main']]],
		['main', 'createLogger', [own, {}]],
	];
	for (const [context, command, arg] of refused) {
		await assert.rejects(Promise.resolve().then(() => facade.call(context, command, arg)), undefined, `${context} ${command} was accepted`);
	}
	assert.throws(() => facade.listen('window:2', 'onDidSomethingElse'), /Event not found/);
	assert.deepEqual(calls, []);
});

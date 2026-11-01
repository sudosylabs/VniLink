// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import test from 'node:test';
import { build } from 'esbuild';

const bundle = await build({
	stdin: { contents: `export { recoverRenderer } from './src/platform/windows/electron-main/rendererRecovery.js';`, loader: 'ts', resolveDir: process.cwd() },
	bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json',
});

const { recoverRenderer } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

function window() {
	const contents = Object.assign(new EventEmitter(), {
		reloads: 0, reload() {
			this.reloads++;
		}
	});
	return Object.assign(new EventEmitter(), {
		webContents: contents, destroyed: false, isDestroyed() {
			return this.destroyed;
		}
	});
}

const log = () => ({
	errors: [], warnings: [], error(message) {
		this.errors.push(message);
	}, warn(message) {
		this.warnings.push(message);
	}, info() { }
});

test('a crashed renderer is reloaded once, not again if it dies right after, and a clean exit is left alone', () => {
	const subject = window();
	const logService = log();
	let time = 0;
	recoverRenderer(subject, logService, () => time);
	subject.webContents.emit('render-process-gone', {}, { reason: 'crashed', exitCode: 11 });
	assert.equal(subject.webContents.reloads, 1, 'A blank window was left after the renderer crashed');
	time = 5_000;
	subject.webContents.emit('render-process-gone', {}, { reason: 'oom', exitCode: 1 });
	assert.equal(subject.webContents.reloads, 1, 'A renderer that dies on load was reloaded in a loop');
	time = 60_000;
	subject.webContents.emit('render-process-gone', {}, { reason: 'killed', exitCode: 9 });
	assert.equal(subject.webContents.reloads, 2);
	subject.webContents.emit('render-process-gone', {}, { reason: 'clean-exit', exitCode: 0 });
	assert.equal(subject.webContents.reloads, 2);
	assert.equal(logService.errors.length, 5);
	subject.emit('unresponsive');
	assert.deepEqual(logService.warnings, ['[window] the renderer stopped responding']);
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const electron = `
import { EventEmitter } from 'node:events';
export const exposed = {};
export const ipcRenderer = Object.assign(new EventEmitter(), { invoked: [], async invoke(channel) { this.invoked.push(channel); return { windowId: 1, osRelease: 'test', platform: 'darwin', version: 'test', engineAvailable: true, enableSmokeTestDriver: false, logsPath: '/logs', logLevel: 2 }; }, send() { } });
export const contextBridge = { exposeInMainWorld(name, value) { exposed[name] = value; } };
export const webUtils = { getPathForFile: () => '/dropped' };`;

async function loadPreload() {
	const output = await build({
		stdin: { contents: `import './src/preload.ts'; export { exposed, ipcRenderer } from 'electron';`, loader: 'ts', resolveDir: process.cwd() }, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json', plugins: [{
			name: 'electron-fixture', setup(builder) {
				builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
				builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: electron, loader: 'js' }));
			}
		}],
	});
	const posted = [];
	Object.assign(process, { sandboxed: true, contextIsolated: true });
	globalThis.window = { postMessage: (data, origin, ports) => posted.push({ data, origin, ports }) };
	globalThis.location = { origin: 'vnilink://app' };
	const module = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
	return { ...module, posted };
}

test('the preload exposes only prefixed IPC, a bounded port relay and no environment', async () => {
	const { exposed, ipcRenderer, posted } = await loadPreload();
	const globals = exposed.vnilink;
	assert.deepEqual(Object.keys(globals).sort(), ['context', 'ipcMessagePort', 'ipcRenderer', 'process', 'webUtils']);
	assert.deepEqual(Object.keys(globals.webUtils), ['getPathForFile'], 'only the path of a dropped file');
	assert.deepEqual(globals.process.env, {});
	assert.equal((await globals.context.resolveConfiguration()).windowId, 1);
	assert.equal(globals.ipcRenderer.invoke, undefined, 'the page invokes nothing; only the preload asks for its configuration');
	assert.throws(() => globals.ipcMessagePort.acquire('dataChannelPort', 'one'), /Unsupported/);
	assert.throws(() => globals.ipcMessagePort.acquire('vnilink:dataChannelPort', 'not a nonce!'), /refused/);
	const closed = [];
	const port = name => ({ close: () => closed.push(name) });
	ipcRenderer.emit('vnilink:dataChannelPort', { ports: [port('early')] }, 'unknown');
	globals.ipcMessagePort.acquire('vnilink:dataChannelPort', 'one');
	assert.throws(() => globals.ipcMessagePort.acquire('vnilink:dataChannelPort', 'one'), /refused/);
	ipcRenderer.emit('vnilink:dataChannelPort', { ports: [port('unsolicited')] }, 'other');
	ipcRenderer.emit('vnilink:dataChannelPort', { ports: [port('answer')] }, 'one');
	assert.deepEqual(closed, ['unsolicited']);
	assert.equal(posted.length, 1);
	assert.equal(posted[0].data, 'one');
	assert.equal(posted[0].origin, 'vnilink://app');
	ipcRenderer.emit('vnilink:dataChannelPort', { ports: [port('replay')] }, 'one');
	assert.deepEqual(closed, ['unsolicited', 'replay']);
	for (let index = 0; index < 8; index++) {
		globals.ipcMessagePort.acquire('vnilink:dataChannelPort', `pending-${String(index)}`);
	}
	assert.throws(() => globals.ipcMessagePort.acquire('vnilink:dataChannelPort', 'overflow'), /refused/);
	globals.ipcMessagePort.release('vnilink:dataChannelPort', 'pending-0');
	globals.ipcMessagePort.acquire('vnilink:dataChannelPort', 'after-release');
});

test('page listeners get a message\'s arguments, never Electron\'s event with its sender and ports', async () => {
	const { exposed, ipcRenderer } = await loadPreload();
	const received = [];
	const listener = (event, ...args) => received.push({ event, args });
	exposed.vnilink.ipcRenderer.on('vnilink:message', listener);
	ipcRenderer.emit('vnilink:message', { sender: ipcRenderer, ports: [{ close() { } }] }, 'payload');
	assert.deepEqual(received, [{ event: {}, args: ['payload'] }]);
	exposed.vnilink.ipcRenderer.removeListener('vnilink:message', listener);
	ipcRenderer.emit('vnilink:message', {}, 'after');
	assert.equal(received.length, 1, 'removeListener left the listener in place');
});

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
	const globals = exposed.vscode;
	assert.deepEqual(Object.keys(globals).sort(), ['context', 'ipcMessagePort', 'ipcRenderer', 'process', 'webUtils']);
	assert.deepEqual(Object.keys(globals.webUtils), ['getPathForFile'], 'only the path of a dropped file');
	assert.deepEqual(globals.process.env, {});
	assert.equal((await globals.context.resolveConfiguration()).windowId, 1);
	assert.throws(() => globals.ipcRenderer.invoke('engine:command'), /Unsupported/);
	assert.throws(() => globals.ipcMessagePort.acquire('dataChannelPort', 'one'), /Unsupported/);
	assert.throws(() => globals.ipcMessagePort.acquire('vscode:dataChannelPort', 'not a nonce!'), /refused/);
	const closed = [];
	const port = name => ({ close: () => closed.push(name) });
	ipcRenderer.emit('vscode:dataChannelPort', { ports: [port('early')] }, 'unknown');
	globals.ipcMessagePort.acquire('vscode:dataChannelPort', 'one');
	assert.throws(() => globals.ipcMessagePort.acquire('vscode:dataChannelPort', 'one'), /refused/);
	ipcRenderer.emit('vscode:dataChannelPort', { ports: [port('unsolicited')] }, 'other');
	ipcRenderer.emit('vscode:dataChannelPort', { ports: [port('answer')] }, 'one');
	assert.deepEqual(closed, ['unsolicited']);
	assert.equal(posted.length, 1);
	assert.equal(posted[0].data, 'one');
	assert.equal(posted[0].origin, 'vnilink://app');
	ipcRenderer.emit('vscode:dataChannelPort', { ports: [port('replay')] }, 'one');
	assert.deepEqual(closed, ['unsolicited', 'replay']);
	for (let index = 0; index < 8; index++) {
		globals.ipcMessagePort.acquire('vscode:dataChannelPort', `pending-${String(index)}`);
	}
	assert.throws(() => globals.ipcMessagePort.acquire('vscode:dataChannelPort', 'overflow'), /refused/);
});

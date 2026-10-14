import { EventEmitter } from 'node:events';
import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const output = await build({
	stdin: { contents: `export { registerDesktopHandlers } from './src/platform/ipc/electron-main/handlers.js'; export { handlers, opened } from 'electron'; export { defaultConfiguration } from './src/platform/configuration/common/configuration.js';`, loader: 'ts', resolveDir: process.cwd() }, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json', plugins: [{
		name: 'electron-fixture', setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: `export const handlers = new Map(); export const opened = []; export const ipcMain = { handle: (name, callback) => handlers.set(name, callback), removeHandler: name => handlers.delete(name) }; export const app = { isPackaged: false, getVersion: () => 'test' }; export const shell = { openExternal: async url => opened.push(url) }; export const systemPreferences = {}; export const screen = { getAllDisplays: () => [] }; export class MessageChannelMain { constructor() { this.port1 = { close() {} }; this.port2 = { close() {} }; } }` }));
		}
	}]
});

const { registerDesktopHandlers, handlers, opened, defaultConfiguration } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

test('restart IPC rechecks native state and only trusted frames can restart or open documentation', async () => {
	const entry = 'vnilink://app/index.html';
	const frame = { url: entry };
	const sent = [];
	const contents = { mainFrame: frame, send: (...args) => sent.push(args) };
	const window = { webContents: contents, isDestroyed: () => false };
	let required = false;
	let restartReadFails = false;
	let restarts = 0;
	const engine = {
		available: true, execute: async command => {
			if (command.type === 'relayRestartRequired') {
				if (restartReadFails) {
					throw new Error('Engine stopped after commit');
				}
				return { type: 'restartRequired', required };
			}
			return { type: 'done' };
		}
	};
	registerDesktopHandlers(() => window, entry, engine, {}, {}, {}, () => {
		restarts++;
	});
	const trusted = { sender: contents, senderFrame: frame };
	const untrusted = { sender: contents, senderFrame: { url: entry } };
	await assert.rejects(handlers.get('app:restart')(untrusted));
	assert.equal(restarts, 0);
	await handlers.get('app:restart')(trusted);
	assert.equal(restarts, 0);
	required = true;
	await handlers.get('app:restart')(trusted);
	assert.equal(restarts, 1);
	await assert.rejects(handlers.get('app:relay-documentation')(untrusted));
	await handlers.get('app:relay-documentation')(trusted, 'https://arbitrary.example/');
	assert.deepEqual(opened, ['https://docs.iroh.computer/relays/rate-limiting']);
	await handlers.get('engine:command')(trusted, { type: 'setRelayPublic' });
	assert(sent.some(([channel, state]) => channel === 'app:restart-state' && state.required));
	restartReadFails = true;
	assert.deepEqual(await handlers.get('engine:command')(trusted, { type: 'setRelayPublic' }), { type: 'done' });
	await assert.rejects(handlers.get('app:restart')(trusted));
	assert.equal(restarts, 1);
});

test('destroyed windows release channel owners without touching native listener hooks', () => {
	class Contents extends EventEmitter {
		destroyed = false;
		mainFrame = { url: 'vnilink://app/index.html' };

		isDestroyed() {
			return this.destroyed;
		}

		postMessage() { }

		removeListener(name, listener) {
			if (this.destroyed && name === 'did-start-navigation') {
				throw new Error('Object has been destroyed');
			}
			return super.removeListener(name, listener);
		}
	}
	const contents = new Contents();
	const closed = [];
	const window = {
		get id() {
			if (contents.destroyed) {
				throw new Error('Window destroyed');
			}
			return 4;
		}, get webContents() {
			if (contents.destroyed) {
				throw new Error('Window destroyed');
			}
			return contents;
		}, isDestroyed: () => contents.destroyed
	};
	registerDesktopHandlers(() => window, contents.mainFrame.url, { attachChannel() { }, closeChannels: owner => closed.push(owner) }, {}, {}, {}, () => { });
	handlers.get('engine:open-data-channel')({ sender: contents, senderFrame: contents.mainFrame }, { requestId: 'probe', kind: 'video', sessionId: '1', generation: '1' });
	contents.destroyed = true;
	assert.doesNotThrow(() => contents.emit('destroyed'));
	assert.equal(closed.length, 1);
	contents.emit('destroyed');
	assert.equal(closed.length, 1);
});

test('defaults are applied before disk commit, refused runtime changes do not save, and persistence failures restore runtime', async () => {
	const entry = 'vnilink://app/index.html';
	const frame = { url: entry };
	const contents = { mainFrame: frame, send() { } };
	const window = { webContents: contents, isDestroyed: () => false };
	let snapshot = { revision: 0, values: defaultConfiguration, error: null };
	let commits = 0;
	let refuse = false;
	let diskFail = false;
	const calls = [];
	const engine = {
		available: true, execute: async command => {
			calls.push(command);
			if (refuse) {
				throw new Error('Held keys');
			}
			return { type: 'done' };
		}
	};
	const config = {
		get snapshot() {
			return snapshot;
		}, updateValue: async change => {
			commits++;
			if (diskFail) {
				throw new Error('Disk refused');
			}
			snapshot = { ...snapshot, revision: snapshot.revision + 1, values: { ...snapshot.values, [change.key]: change.value } };
			return snapshot;
		}
	};
	registerDesktopHandlers(() => window, entry, engine, {}, {}, config, () => { });
	const event = { sender: contents, senderFrame: frame };
	const change = { key: 'input.defaults', value: { ...defaultConfiguration['input.defaults'], mode: 'Map' } };
	refuse = true;
	await assert.rejects(handlers.get('configuration:update')(event, change));
	assert.equal(commits, 0);
	refuse = false;
	diskFail = true;
	await assert.rejects(handlers.get('configuration:update')(event, change));
	assert.equal(calls.at(-1).mode, 'Character');
	assert.equal(snapshot.values['input.defaults'].mode, 'Character');
	diskFail = false;
	await handlers.get('configuration:update')(event, change);
	assert.equal(snapshot.values['input.defaults'].mode, 'Map');
	assert.equal(calls.at(-1).mode, 'Map');
	const codec = { key: 'desktop.codec', value: 'av1' };
	diskFail = true;
	await assert.rejects(handlers.get('configuration:update')(event, codec));
	assert.deepEqual(calls.at(-1), { type: 'setDesktopCodecDefault', codec: 'auto' });
	assert.equal(snapshot.values['desktop.codec'], 'auto');
	diskFail = false;
	await handlers.get('configuration:update')(event, codec);
	assert.deepEqual(calls.at(-1), { type: 'setDesktopCodecDefault', codec: 'av1' });
	assert.equal(snapshot.values['desktop.codec'], 'av1');
});

test('IPC owner releases registrations and rejects commands after disposal', async () => {
	const frame = { url: 'vnilink://app/index.html' };
	const contents = { mainFrame: frame };
	const controller = registerDesktopHandlers(() => ({ webContents: contents }), frame.url, { available: false }, {}, {}, { snapshot: { values: defaultConfiguration } }, () => { });
	const handler = handlers.get('configuration:get');
	assert.equal(handler({ sender: contents, senderFrame: frame }).values, defaultConfiguration);
	controller.dispose();
	assert.equal(handlers.size, 0);
	assert.throws(() => handler({ sender: contents, senderFrame: frame }), /disposed/);
});

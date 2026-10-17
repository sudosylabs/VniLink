import { EventEmitter } from 'node:events';
import assert from 'node:assert/strict';
import { join } from 'node:path';
import test from 'node:test';
import { build } from 'esbuild';

const electron = `
export const handlers = new Map();
export const opened = [];
export const windows = new Map();
export const ipcMain = { handle: (name, callback) => handlers.set(name, callback), removeHandler: name => handlers.delete(name), on() { }, once() { }, removeListener() { } };
export const app = { isPackaged: false, getVersion: () => 'test', getPath: () => '/Pictures' };
export const shell = { openExternal: async url => opened.push(url) };
export const dialog = { showOpenDialog: async () => ({ canceled: true, filePaths: [] }) };
export const systemPreferences = {};
export const screen = { getAllDisplays: () => [] };
export const nativeTheme = { on() { }, removeListener() { } };
export const utilityProcess = {};
export const BrowserWindow = { fromWebContents: contents => windows.get(contents) ?? null };
export class MessageChannelMain { constructor() { this.port1 = { close() { } }; this.port2 = { close() { } }; } }
export default { ipcMain, BrowserWindow };`;

/** Replaces the settings file store so commits can be refused on demand. */
const store = `
import { Emitter } from '${process.cwd()}/src/base/common/event.js';
import { defaultConfiguration } from '${process.cwd()}/src/platform/configuration/common/configurationSnapshot.js';
export const disk = { fail: false, commits: 0, values: defaultConfiguration };
export class ConfigurationStore {
	onDidChangeConfiguration = new Emitter().event;
	get snapshot() { return { revision: disk.commits, values: disk.values, error: null }; }
	async updateValue(change) { disk.commits++; if (disk.fail) { throw new Error('Disk refused'); } disk.values = { ...disk.values, [change.key]: change.value }; return this.snapshot; }
	dispose() { }
}`;

const output = await build({
	stdin: {
		contents: `
			export { validatedIpcMain, WORKBENCH_URL } from './src/base/parts/ipc/electron-main/ipcMain.js';
			export { NativeHostChannel } from './src/platform/native/electron-main/nativeHostChannel.js';
			export { NativeHostMainService } from './src/platform/native/electron-main/nativeHostMainService.js';
			export { EngineChannel } from './src/platform/engine/electron-main/engineChannel.js';
			export { ConfigurationMainService } from './src/platform/configuration/electron-main/configurationMainService.js';
			export { RecordingMainService } from './src/platform/recording/electron-main/recordingMainService.js';
			export { windowIdFromContext } from './src/platform/ipc/electron-main/windowContext.js';
			export { setUnexpectedErrorHandler } from './src/base/common/errors.js';
			export { defaultConfiguration } from './src/platform/configuration/common/configurationSnapshot.js';
			export { Event } from './src/base/common/event.js';
			export { handlers, opened, windows } from 'electron';
			export { disk } from 'fixture:store';`,
		loader: 'ts', resolveDir: process.cwd(),
	}, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json', plugins: [{
		name: 'main-fixtures', setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
			builder.onResolve({ filter: /^fixture:store$|\/configurationStore\.js$/ }, () => ({ path: 'store', namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: path === 'electron' ? electron : store, loader: 'js', resolveDir: process.cwd() }));
		}
	}]
});

const {
	validatedIpcMain, WORKBENCH_URL, NativeHostChannel, NativeHostMainService, EngineChannel, ConfigurationMainService, RecordingMainService,
	windowIdFromContext, setUnexpectedErrorHandler, defaultConfiguration, Event, handlers, opened, windows, disk,
} = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

const logService = { error() { }, trace() { } };

// Refused IPC events are reported, not thrown, as the application's handler does.
const unexpected = [];

setUnexpectedErrorHandler(error => unexpected.push(error));

function appWindow(id = 4) {
	const contents = new EventEmitter();
	contents.mainFrame = { url: WORKBENCH_URL, parent: null };
	contents.isDestroyed = () => false;
	contents.postMessage = () => { };
	const window = { id, webContents: contents, isDestroyed: () => false };
	windows.set(contents, window);
	return window;
}

test('privileged channels accept only the workbench main frame of an application window', async () => {
	const window = appWindow();
	let calls = 0;
	validatedIpcMain.handle('vscode:probe', () => {
		calls++;
	});
	const handler = handlers.get('vscode:probe');
	const contents = window.webContents;
	await handler({ sender: contents, senderFrame: contents.mainFrame });
	assert.equal(calls, 1);
	for (const senderFrame of [{ url: 'about:blank', parent: null }, { url: undefined, parent: null }, { url: WORKBENCH_URL, parent: {} }, { url: 'vnilink://app/other.html', parent: null }]) {
		await assert.rejects(handler({ sender: contents, senderFrame }), /Invalid channel/);
	}
	const stranger = new EventEmitter();
	stranger.mainFrame = { url: WORKBENCH_URL, parent: null };
	await assert.rejects(handler({ sender: stranger, senderFrame: stranger.mainFrame }), /Invalid channel/);
	assert.equal(calls, 1, 'An untrusted sender reached a privileged handler');
	assert.equal(unexpected.length, 5);
	assert.throws(() => windowIdFromContext('window:0'));
	assert.throws(() => windowIdFromContext('renderer'));
	assert.equal(windowIdFromContext('window:12'), 12);
});

test('native host requests must name their own window and links are allowlisted', async () => {
	const window = appWindow(7);
	let relaunches = 0;
	const nativeHost = new NativeHostMainService({ onDidOpenWindow: Event.None, getWindowById: id => id === 7 ? window : undefined });
	const channel = new NativeHostChannel(nativeHost, async () => {
		relaunches++;
	});
	await assert.rejects(channel.call('window:7', 'getWindowState', [8]), /Invalid/);
	await assert.rejects(channel.call('window:7', 'invalid', [7]), /Invalid/);
	await assert.rejects(channel.call('renderer', 'relaunch', [7]));
	await channel.call('window:7', 'relaunch', [7]);
	assert.equal(relaunches, 1);
	await assert.rejects(channel.call('window:7', 'openExternal', [7, 'https://arbitrary.example/']), /not allowed/);
	await channel.call('window:7', 'openExternal', [7, 'https://docs.iroh.computer/relays/rate-limiting']);
	assert.deepEqual(opened, ['https://docs.iroh.computer/relays/rate-limiting']);
});

test('relay commands refresh restart state, and a failed readback still reports the committed command', async () => {
	let refreshes = 0;
	let refreshFails = false;
	const engine = {
		onDidChangeAvailability: Event.None, onDidChangeRestartRequired: Event.None, execute: async () => ({ type: 'done' }),
		refreshRestartRequired: async () => {
			refreshes++;
			if (refreshFails) {
				throw new Error('Engine stopped after commit');
			}
		},
	};
	const channel = new EngineChannel(engine, {}, {}, {});
	await assert.rejects(channel.call('window:1', 'execute', { type: 'shutdown' }), /Invalid/);
	for (const command of [{ type: 'subscribeEvents' }, { type: 'pollEvents', subscriptionId: '1', maxEvents: 1 }, { type: 'releaseEvents', subscriptionId: '1' }]) {
		await assert.rejects(channel.call('window:1', 'execute', command), /Invalid/, `${command.type} must stay private to main`);
	}
	await channel.call('window:1', 'execute', { type: 'setRelayPublic' });
	assert.equal(refreshes, 1);
	refreshFails = true;
	assert.deepEqual(await channel.call('window:1', 'execute', { type: 'setRelayPublic' }), { type: 'done' });
	await channel.call('window:1', 'execute', { type: 'snapshot' });
	assert.equal(refreshes, 2);
	channel.dispose();
});

test('destroyed windows release channel owners once without touching native listener hooks', async () => {
	class Contents extends EventEmitter {
		destroyed = false;
		mainFrame = { url: WORKBENCH_URL };

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
	const window = { id: 4, webContents: contents };
	const closed = [];
	const channel = new EngineChannel({ attachChannel() { }, closeChannels: owner => closed.push(owner) }, { getWindowById: id => id === 4 ? window : undefined }, {}, {});
	await channel.call('window:4', 'openDataChannel', { requestId: 'probe', kind: 'video', sessionId: '1', generation: '1' });
	contents.destroyed = true;
	assert.doesNotThrow(() => contents.emit('destroyed'));
	assert.equal(closed.length, 1);
	contents.emit('destroyed');
	assert.equal(closed.length, 1);
	await assert.rejects(channel.call('window:5', 'openDataChannel', { requestId: 'probe', kind: 'video', sessionId: '1', generation: '1' }));
	channel.dispose();
});

test('defaults reach the engine before commit, refused changes do not save, and failed commits restore the engine', async () => {
	const calls = [];
	let refuse = false;
	const engine = {
		available: true, execute: async command => {
			calls.push(command);
			if (refuse) {
				throw new Error('Held keys');
			}
			return { type: 'done' };
		}
	};
	disk.values = defaultConfiguration;
	disk.commits = 0;
	const configuration = new ConfigurationMainService('/settings.json', engine, logService);
	const change = { key: 'input.defaults', value: { ...defaultConfiguration['input.defaults'], mode: 'Map' } };
	refuse = true;
	await assert.rejects(configuration.updateValue(change));
	assert.equal(disk.commits, 0);
	refuse = false;
	disk.fail = true;
	await assert.rejects(configuration.updateValue(change));
	assert.equal(calls.at(-1).mode, 'Character');
	assert.equal(configuration.snapshot.values['input.defaults'].mode, 'Character');
	disk.fail = false;
	await configuration.updateValue(change);
	assert.equal(configuration.snapshot.values['input.defaults'].mode, 'Map');
	assert.equal(calls.at(-1).mode, 'Map');
	const codec = { key: 'desktop.codec', value: 'av1' };
	disk.fail = true;
	await assert.rejects(configuration.updateValue(codec));
	assert.deepEqual(calls.at(-1), { type: 'setDesktopCodecDefault', codec: 'auto' });
	disk.fail = false;
	await configuration.updateValue(codec);
	assert.deepEqual(calls.at(-1), { type: 'setDesktopCodecDefault', codec: 'av1' });
	configuration.dispose();
});

test('media defaults roll back partial runtime writes without publishing a false saved preference', async () => {
	const commands = [];
	let refused = false;
	const engine = {
		available: true, execute: command => {
			commands.push(command);
			if (command.type === 'setNativeAudioSources' && command.microphones.length && !refused) {
				refused = true;
				return Promise.reject(new Error('Audio source unavailable'));
			}
			return Promise.resolve({ type: 'done' });
		}
	};
	disk.values = defaultConfiguration;
	const configuration = new ConfigurationMainService('/settings.json', engine, logService);
	await assert.rejects(configuration.updateValue({ key: 'media.sources', value: { cameras: ['camera'], microphones: ['mic'], system: [] } }), /Audio source unavailable/);
	assert.deepEqual(configuration.snapshot.values['media.sources'], defaultConfiguration['media.sources']);
	assert.deepEqual(commands.map(command => command.type), ['setNativeCameraSources', 'setNativeAudioSources', 'setNativeCameraSources', 'setNativeAudioSources']);
	assert.deepEqual(commands[2].ids, defaultConfiguration['media.sources'].cameras);
	configuration.dispose();
});

test('the default recording folder stays independent of a saved custom folder', async () => {
	const custom = { ...defaultConfiguration, 'recording.defaults': { ...defaultConfiguration['recording.defaults'], directory: '/custom-recordings' } };
	const recording = new RecordingMainService({ snapshot: { values: custom, error: null } });
	assert.equal(await recording.getDefaultDirectory(), join('/Pictures', 'VniLink'));
	const broken = new RecordingMainService({ snapshot: { values: custom, error: 'Settings could not be loaded.' } });
	await assert.rejects(broken.createDestination(), /could not be loaded/);
});

// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { EventEmitter } from 'node:events';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { build } from 'esbuild';

const electron = `
export const handlers = new Map();
export const opened = [];
export const windows = new Map();
export const ipcMain = { handle: (name, callback) => handlers.set(name, callback), removeHandler: name => handlers.delete(name), on() { }, once() { }, removeListener() { } };
export const app = { isPackaged: false, getVersion: () => 'test', getPath: () => '/Pictures', getLoginItemSettings: () => ({ openAtLogin: false }) };
export const shown = [];
export const shell = { openExternal: async url => opened.push(url), showItemInFolder: path => shown.push(path) };
export const clipboard = { text: '', items: [], readText: async () => clipboard.text, writeText: async text => { clipboard.text = text; }, write: async items => { clipboard.items = items; } };
export class ClipboardItem { constructor(items) { this.items = items; } }
export const dialog = { showOpenDialog: async () => ({ canceled: true, filePaths: [] }) };
export const systemPreferences = {};
export const screen = { getAllDisplays: () => [] };
export const nativeTheme = { on() { }, removeListener() { } };
export const utilityProcess = {};
export const BrowserWindow = { fromWebContents: contents => windows.get(contents) ?? null };
export class MessageChannelMain { constructor() { this.port1 = { close() { } }; this.port2 = { close() { } }; } }
export default { ipcMain, BrowserWindow };`;

/** Replaces settings.json so commits can be refused on demand. */
// Forward slashes: these paths are spliced into JavaScript source, where Windows backslashes would be escapes.
const root = process.cwd().replaceAll('\\', '/');

const store = `
import { Emitter } from '${root}/src/base/common/event.js';
import { settingDefaults } from '${root}/src/platform/configuration/common/configurationSchema.js';
export const disk = { fail: false, commits: 0, file: {} };
const changed = new Emitter();
export class UserSettings {
	onDidChange = changed.event;
	error = undefined;
	get values() { return { ...settingDefaults('user'), ...disk.file }; }
	async update(key, value) { disk.commits++; if (disk.fail) { throw new Error('Disk refused'); } disk.file = { ...disk.file, [key]: value }; changed.fire(); }
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
			export { UIStateChannel } from './src/platform/state/electron-main/uiStateChannel.js';
			export { SaveStrategy, StateService } from './src/platform/state/node/stateService.js';
			export { windowIdFromContext } from './src/platform/ipc/electron-main/windowContext.js';
			export { setUnexpectedErrorHandler } from './src/base/common/errors.js';
			export { Event } from './src/base/common/event.js';
			export { handlers, opened, shown, windows, clipboard } from 'electron';
			export { VSBuffer } from './src/base/common/buffer.js';
			export { disk } from 'fixture:store';`,
		loader: 'ts', resolveDir: process.cwd(),
	}, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json', plugins: [{
		name: 'main-fixtures', setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
			builder.onResolve({ filter: /^fixture:store$|\/userSettings\.js$/ }, () => ({ path: 'store', namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: path === 'electron' ? electron : store, loader: 'js', resolveDir: process.cwd() }));
		}
	}]
});

const {
	validatedIpcMain, WORKBENCH_URL, NativeHostChannel, NativeHostMainService, EngineChannel, ConfigurationMainService, RecordingMainService,
	UIStateChannel, SaveStrategy, StateService,
	windowIdFromContext, setUnexpectedErrorHandler, Event, handlers, opened, shown, windows, disk, clipboard, VSBuffer,
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

test('a diagnostics export reads the end of this run\'s main and window logs, home written ~', async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-logs-'));
	try {
		await mkdir(join(root, 'window1'), { recursive: true });
		await writeFile(join(root, 'main.log'), `started\nopened ${homedir()}/Library/x\n`);
		await writeFile(join(root, 'window1', 'renderer.log'), 'x'.repeat(300 * 1024) + 'last');
		const window = appWindow(7);
		const nativeHost = new NativeHostMainService({ fsPath: root }, { onDidOpenWindow: Event.None, getWindowById: id => id === 7 ? window : undefined });
		const channel = new NativeHostChannel(nativeHost, async () => { });
		await assert.rejects(channel.call('window:7', 'readLogTails', [7, 'main.log']), /Invalid/);
		const tails = await channel.call('window:7', 'readLogTails', [7]);
		assert.deepEqual(tails.map(tail => tail.name).sort(), ['main.log', join('window1', 'renderer.log')]);
		const main = tails.find(tail => tail.name === 'main.log');
		assert.equal(main.text, 'started\nopened ~/Library/x\n');
		const renderer = tails.find(tail => tail.name !== 'main.log');
		assert.equal(renderer.text.length, 256 * 1024);
		assert.ok(renderer.text.endsWith('last'));
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('native host requests must name their own window and links are allowlisted', async () => {
	const window = appWindow(7);
	let relaunches = 0;
	const nativeHost = new NativeHostMainService({ fsPath: '/logs/run' }, { onDidOpenWindow: Event.None, getWindowById: id => id === 7 ? window : undefined });
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
	await assert.rejects(channel.call('window:7', 'openLogsFolder', [7, '/elsewhere']), /Invalid/);
	await channel.call('window:7', 'openLogsFolder', [7]);
	assert.deepEqual(shown, [join('/logs/run', 'main.log')]);
	// Plain text only, for the window that asks, within a bound.
	await channel.call('window:7', 'writeClipboardText', [7, 'copied ✓']);
	assert.equal(await channel.call('window:7', 'readClipboardText', [7]), 'copied ✓');
	assert.equal(await channel.call('window:7', 'readClipboardText', [7, 'selection']), 'copied ✓', 'The selection clipboard did not fall back off Linux');
	for (const args of [[8, 'other window'], [7, 42], [7, 'x'.repeat(16 * 1024 * 1024 + 1)], [7, 'text', 'find'], [7, 'text', 'clipboard', 'extra']]) {
		await assert.rejects(channel.call('window:7', 'writeClipboardText', args), /Invalid/);
	}
	await assert.rejects(channel.call('window:7', 'readClipboardText', [7, 'files']), /Invalid/);
	await assert.rejects(channel.call('window:7', 'readClipboardText', [8]), /Invalid/);
	assert.equal(await channel.call('window:7', 'readClipboardText', [7]), 'copied ✓', 'A refused write reached the clipboard');
});

test('a window copies a PNG picture, and nothing else, to the clipboard', async () => {
	const window = appWindow(7);
	const nativeHost = new NativeHostMainService({ fsPath: '/logs/run' }, { onDidOpenWindow: Event.None, getWindowById: id => id === 7 ? window : undefined });
	const channel = new NativeHostChannel(nativeHost, async () => { });
	const png = VSBuffer.wrap(Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]));
	await channel.call('window:7', 'writeClipboardPicture', [7, png]);
	assert.deepEqual(clipboard.items.map(item => Object.keys(item.items)), [['image/png']]);
	await assert.rejects(channel.call('window:7', 'writeClipboardPicture', [7, VSBuffer.fromString('not a picture')]), /Invalid/);
	await assert.rejects(channel.call('window:7', 'writeClipboardPicture', [8, png]), /Invalid/);
	await assert.rejects(channel.call('window:7', 'savePicture', [7, 'picture.png', png]), /Invalid/);
});

test('windows change settings only through the configuration channel', async () => {
	const executed = [];
	const engine = {
		onDidChangeAvailability: Event.None, onDidChangeRestartRequired: Event.None, execute: async command => {
			executed.push(command);
			return { type: 'done' };
		}
	};
	const channel = new EngineChannel(engine, {}, {}, {});
	await assert.rejects(channel.call('window:1', 'execute', { type: 'shutdown' }), /Invalid/);
	const privateCommands = [
		{ type: 'subscribeEvents' }, { type: 'pollEvents', subscriptionId: '1', maxEvents: 1 }, { type: 'releaseEvents', subscriptionId: '1' },
		{ type: 'setRelayPublic' }, { type: 'setSharingPolicy', policy: { view: true } }, { type: 'setDesktopCodecDefault', codec: 'auto' }, { type: 'setTransferDirectory', path: '/tmp/Transfers' }, { type: 'setTerminalShell', shell: null },
		{ type: 'setLogFilter', directives: 'off' },
	];
	for (const command of privateCommands) {
		await assert.rejects(channel.call('window:1', 'execute', command), /Invalid/, `${command.type} must stay private to main`);
	}
	assert.deepEqual(await channel.call('window:1', 'execute', { type: 'snapshot' }), { type: 'done' });
	assert.deepEqual(executed, [{ type: 'snapshot' }]);
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
	const engine = { attachChannel() { }, closeChannels: owner => closed.push(owner) };
	const channel = new EngineChannel(engine, { getWindowById: id => id === 4 ? window : undefined }, {}, {});
	await channel.call('window:4', 'openDataChannel', { requestId: 'probe', kind: 'video', sessionId: '1', generation: '1' });
	contents.destroyed = true;
	assert.doesNotThrow(() => contents.emit('destroyed'));
	assert.equal(closed.length, 1);
	contents.emit('destroyed');
	assert.equal(closed.length, 1);
	await assert.rejects(channel.call('window:5', 'openDataChannel', { requestId: 'probe', kind: 'video', sessionId: '1', generation: '1' }));
	channel.dispose();
});

function settingsEngine(execute) {
	return { available: true, onDidChangeAvailability: Event.None, execute, refreshRestartRequired: async () => { } };
}

test('defaults reach the engine before commit, refused changes do not save, and failed commits restore the engine', async () => {
	const calls = [];
	let refuse = false;
	const engine = settingsEngine(async command => {
		calls.push(command);
		if (refuse) {
			throw new Error('Held keys');
		}
		return { type: 'done' };
	});
	disk.file = {};
	disk.commits = 0;
	const configuration = new ConfigurationMainService('/settings.json', engine, logService);
	refuse = true;
	await assert.rejects(configuration.updateValue('input.keyboardMode', 'Map'));
	assert.equal(disk.commits, 0);
	refuse = false;
	disk.fail = true;
	await assert.rejects(configuration.updateValue('input.keyboardMode', 'Map'));
	assert.equal(calls.at(-1).mode, 'Character');
	assert.equal(configuration.snapshot.values['input.keyboardMode'], 'Character');
	disk.fail = false;
	await configuration.updateValue('input.keyboardMode', 'Map');
	assert.equal(configuration.snapshot.values['input.keyboardMode'], 'Map');
	assert.deepEqual(calls.at(-1), { type: 'setInputDefaults', mode: 'Map', swap: false, pointer: { sensitivity_percent: 100, reverse_wheel: false, swap_buttons: false } });
	disk.fail = true;
	await assert.rejects(configuration.updateValue('desktop.codec', 'av1'));
	assert.deepEqual(calls.at(-1), { type: 'setDesktopCodecDefault', codec: 'auto' });
	disk.fail = false;
	await configuration.updateValue('desktop.codec', 'av1');
	assert.deepEqual(calls.at(-1), { type: 'setDesktopCodecDefault', codec: 'av1' });
	await assert.rejects(configuration.updateValue('desktop.codec', 'bogus'), /available options/);
	await assert.rejects(configuration.updateValue('unknown.setting', true), /Unknown setting/);
	configuration.dispose();
});

test('media defaults roll back partial runtime writes without publishing a false saved preference', async () => {
	const commands = [];
	let refused = false;
	const engine = settingsEngine(command => {
		commands.push(command);
		if (command.type === 'setNativeAudioSources' && command.microphones.includes('mic') && !refused) {
			refused = true;
			return Promise.reject(new Error('Audio source unavailable'));
		}
		return Promise.resolve({ type: 'done' });
	});
	disk.file = {};
	const configuration = new ConfigurationMainService('/settings.json', engine, logService);
	await assert.rejects(configuration.updateValue('media.microphone', 'mic'), /Audio source unavailable/);
	assert.equal(configuration.snapshot.values['media.microphone'], 'vnilink:default-microphone');
	assert.deepEqual(commands.map(command => command.type), ['setNativeAudioSources', 'setNativeAudioSources']);
	assert.deepEqual(commands[1], { type: 'setNativeAudioSources', microphones: ['vnilink:default-microphone'], system: ['vnilink:default-system-audio'] });
	configuration.dispose();
});

test('the offered shell is chosen by id, refused while shells run, and a missing saved shell stays saved', async () => {
	const commands = [];
	let refusal;
	const engine = settingsEngine(async command => {
		commands.push(command);
		if (command.type === 'setTerminalShell' && refusal) {
			throw new Error(refusal);
		}
		return { type: 'done' };
	});
	const warnings = [];
	const log = { error() { }, trace() { }, warn: (...args) => warnings.push(args) };
	disk.file = {};
	const configuration = new ConfigurationMainService('/settings.json', engine, log);
	await configuration.updateValue('terminal.shell', '/bin/zsh');
	assert.deepEqual(commands.at(-1), { type: 'setTerminalShell', shell: '/bin/zsh' });
	assert.equal(configuration.snapshot.values['terminal.shell'], '/bin/zsh');
	// Busy: hosted shells keep running, nothing is saved, and the message says what to do.
	refusal = 'Busy: the host already has a controlling session';
	await assert.rejects(configuration.updateValue('terminal.shell', 'git-bash'), /Close hosted terminals before changing the shell/);
	assert.equal(configuration.snapshot.values['terminal.shell'], '/bin/zsh');
	refusal = 'InvalidSetting: that shell is not installed on this computer';
	await assert.rejects(configuration.updateValue('terminal.shell', 'gone'), /not installed/);
	refusal = undefined;
	await configuration.updateValue('terminal.shell', '');
	assert.deepEqual(commands.at(-1), { type: 'setTerminalShell', shell: null }, 'Automatic was not sent as null');
	await assert.rejects(configuration.updateValue('terminal.shell', 'a\0b'), /installed on this computer/);
	configuration.dispose();

	// At startup a saved shell is offered again; one no longer installed leaves
	// the automatic shell in place but stays saved for when it returns.
	disk.file = { 'terminal.shell': 'fish' };
	commands.length = 0;
	refusal = 'InvalidSetting: that shell is not installed on this computer';
	const restored = new ConfigurationMainService('/settings.json', engine, log);
	await restored.restoreEngineDefaults(false);
	assert.deepEqual(commands.filter(command => command.type === 'setTerminalShell'), [{ type: 'setTerminalShell', shell: 'fish' }]);
	assert.equal(warnings.length, 1);
	assert.equal(restored.snapshot.values['terminal.shell'], 'fish');
	commands.length = 0;
	await restored.restoreEngineDefaults(true);
	assert(!commands.some(command => command.type === 'setTerminalShell'), 'A temporary engine was given a host shell');
	restored.dispose();
});

test('the engine log setting sends its filter, the advanced one first, and a refused one is not saved', async () => {
	const commands = [];
	const engine = settingsEngine(async command => {
		commands.push(command);
		if (command.type === 'setLogFilter' && command.directives === 'vnilink=loud') {
			throw new Error('InvalidArg: The log filter is not valid');
		}
		return { type: 'logFilter', directives: command.directives ?? 'warn,vnilink=info', source: command.directives === null ? 'default' : 'setting' };
	});
	const errors = [];
	const log = { error: (...args) => errors.push(args), trace() { }, warn() { } };
	disk.file = {};
	const configuration = new ConfigurationMainService('/settings.json', engine, log);
	await configuration.updateValue('diagnostics.engineLog', 'debug');
	assert.deepEqual(commands.at(-1), { type: 'setLogFilter', directives: 'warn,vnilink=debug' });
	await configuration.updateValue('diagnostics.engineLog', 'off');
	assert.deepEqual(commands.at(-1), { type: 'setLogFilter', directives: 'off' });
	await configuration.updateValue('diagnostics.engineLogFilter', ' vnilink::files=trace ');
	assert.deepEqual(commands.at(-1), { type: 'setLogFilter', directives: 'vnilink::files=trace' });
	await assert.rejects(configuration.updateValue('diagnostics.engineLogFilter', 'vnilink=loud'), /not valid/);
	assert.equal(configuration.snapshot.values['diagnostics.engineLogFilter'], ' vnilink::files=trace ');
	assert.deepEqual(commands.at(-1), { type: 'setLogFilter', directives: 'vnilink::files=trace' }, 'the previous filter was not restored');
	await configuration.updateValue('diagnostics.engineLogFilter', '');
	await configuration.updateValue('diagnostics.engineLog', 'info');
	assert.deepEqual(commands.at(-1), { type: 'setLogFilter', directives: null }, 'the default level was not sent as the engine default');
	await assert.rejects(configuration.updateValue('diagnostics.engineLogFilter', 'x'.repeat(1025)), /shorter filter/);
	configuration.dispose();

	// A saved filter the engine refuses at startup leaves its default and is logged.
	disk.file = { 'diagnostics.engineLogFilter': 'vnilink=loud' };
	commands.length = 0;
	const restored = new ConfigurationMainService('/settings.json', engine, log);
	await restored.restoreEngineDefaults(false);
	assert.deepEqual(commands.filter(command => command.type === 'setLogFilter'), [{ type: 'setLogFilter', directives: 'vnilink=loud' }]);
	assert.equal(errors.length, 1);
	restored.dispose();
});

test('the default recording folder stays independent of a saved custom folder', async () => {
	const recording = new RecordingMainService({ getValue: () => '/custom-recordings' });
	assert.equal(await recording.getDefaultDirectory(), join('/Pictures', 'VniLink'));
});

test('a picture lands in the recordings folder under its own name, never over another', async () => {
	const folder = await mkdtemp(join(tmpdir(), 'vnilink-pictures-'));
	try {
		const recording = new RecordingMainService({ getValue: () => folder });
		const png = VSBuffer.wrap(Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]));
		await recording.savePicture('Studio 2026-10-19 14.32.05.png', png);
		await recording.savePicture('Studio 2026-10-19 14.32.05.png', png);
		assert.deepEqual([...await readFile(join(folder, 'Studio 2026-10-19 14.32.05.png'))], [...png.buffer]);
		assert.deepEqual([...await readFile(join(folder, 'Studio 2026-10-19 14.32.05 (2).png'))], [...png.buffer]);
		// Arguments come from the window unchecked: a path, a non-PNG or a non-buffer is refused.
		for (const [name, bytes] of [['../escape.png', png], ['nested/picture.png', png], ['picture.png', VSBuffer.fromString('not a picture')], ['picture.png', Uint8Array.from(png.buffer)]]) {
			await assert.rejects(recording.savePicture(name, bytes), /Invalid picture/);
		}
	} finally {
		await rm(folder, { recursive: true, force: true });
	}
});

test('windows remember only known UI state, in its own shape, in the state file', async () => {
	const folder = await mkdtemp(join(tmpdir(), 'vnilink-state-'));
	const path = join(folder, 'storage.json');
	try {
		const state = new StateService(SaveStrategy.DELAYED, path, logService);
		await state.init();
		state.setItem('windowsState', { main: true });
		const channel = new UIStateChannel(state);
		const lastFolder = 'files.lastFolder.k51qzi5uqu5d-studio_2.remote';
		const folderValue = { root: '0', path: 'Users/someone/Documents' };
		await assert.rejects(channel.call('renderer', 'set', { key: 'files.showHidden', value: true }), /window/i);
		await channel.call('window:1', 'set', { key: lastFolder, value: folderValue });
		await channel.call('window:1', 'set', { key: 'files.showHidden', value: true });
		assert.deepEqual(await channel.call('window:1', 'get', { key: lastFolder }), folderValue);
		assert.equal(await channel.call('window:1', 'get', { key: 'files.showHidden' }), true);
		assert.equal(await channel.call('window:1', 'get', { key: 'files.lastFolder.other.local' }), undefined);
		// A remote desktop remembers, for each computer, whether it exchanges Control and Command.
		await channel.call('window:1', 'set', { key: 'desktop.swapModifiers.studio', value: false });
		assert.equal(await channel.call('window:1', 'get', { key: 'desktop.swapModifiers.studio' }), false);
		await assert.rejects(channel.call('window:1', 'set', { key: 'desktop.swapModifiers.studio', value: 'off' }), /Invalid UI state value/);

		// The main process's own keys, unknown keys and malformed values are refused, and nothing is written for them.
		for (const key of ['windowsState', 'files.lastFolder.studio.both', 'files.lastFolder..local', 'files.lastFolder.a/b.local', `files.lastFolder.${'x'.repeat(129)}.local`, 'files.showHidden.extra', 42]) {
			await assert.rejects(channel.call('window:1', 'get', { key }), /Invalid UI state key/, String(key));
			await assert.rejects(channel.call('window:1', 'set', { key, value: true }), /Invalid UI state value/, String(key));
		}
		for (const value of ['yes', 1, null, undefined, { root: '0', path: 'a', extra: 1 }]) {
			await assert.rejects(channel.call('window:1', 'set', { key: 'files.showHidden', value }), /Invalid UI state value/);
		}
		for (const value of [true, '/Users', { root: '', path: 'a' }, { root: 1, path: 'a' }, { root: '0' }, { root: '0', path: 'a'.repeat(4097) }, { root: '0', path: 'a', extra: 1 }]) {
			await assert.rejects(channel.call('window:1', 'set', { key: lastFolder, value }), /Invalid UI state value/);
		}
		await assert.rejects(channel.call('window:1', 'get', { key: lastFolder, value: 1 }), /Invalid UI state key/);
		await assert.rejects(channel.call('window:1', 'clear', {}), /Unknown UI state command/);
		assert.throws(() => channel.listen('window:1', 'onDidChange'), /Unknown UI state event/);

		// Saved values survive a restart; a removed key is gone and the main process's keys are untouched.
		await channel.call('window:1', 'remove', { key: 'files.showHidden' });
		await state.close();
		state.dispose();
		const reopened = new StateService(SaveStrategy.DELAYED, path, logService);
		await reopened.init();
		assert.deepEqual(reopened.getItem(lastFolder), folderValue);
		assert.equal(reopened.getItem('files.showHidden'), undefined);
		assert.deepEqual(reopened.getItem('windowsState'), { main: true });
		reopened.dispose();
	} finally {
		await rm(folder, { recursive: true, force: true });
	}
});

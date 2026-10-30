// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const root = process.cwd().replaceAll('\\', '/');

const electron = `import { EventEmitter } from 'node:events';
export const state = { attempts: 0, completed: false, relaunched: 0, exitCode: undefined, windows: 0 };
export const app = new EventEmitter();
Object.assign(app, {
	isPackaged: true, getPath: () => '/fixture', getVersion: () => '0.0.0', setName() { }, requestSingleInstanceLock: () => true, whenReady: () => Promise.resolve(),
	relaunch() { state.relaunched++; },
	quit() { state.attempts++; let prevented = false; this.emit('before-quit', { preventDefault() { prevented = true; } }); if (!prevented) { state.completed = true; this.emit('will-quit'); } },
	exit(code) { state.exitCode = code; },
});
export const protocol = { registerSchemesAsPrivileged() { }, handle() { } };
export const session = { defaultSession: { setPermissionRequestHandler() { }, setPermissionCheckHandler() { } } };
export const nativeTheme = {};
export const net = {};
export const BrowserWindow = { getAllWindows: () => [], fromWebContents: () => null };
export const Menu = class { static setApplicationMenu() {} static buildFromTemplate() { return {}; } };
export const MenuItem = class {};
export class MessageChannelMain { }`;

/** Main services the composition test replaces; each records only what the test asserts. */
const services = {
	engineMainService: `import { Event } from '${root}/src/base/common/event.js'; import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js'; import { app } from 'electron';
		export const IEngineMainService = createDecorator('engineMainService');
		export const shutdown = { stops: 0 }; let release; export const stopped = new Promise(resolve => { release = resolve; }); export function finish() { release(); }
		export class EngineMainService { available = true; onDidChangeAvailability = Event.None; onDidChangeRestartRequired = Event.None; async start() { app.emit('activate'); } async refreshRestartRequired() { } async execute(command) { return command.type === 'nativeRuntime' ? { type: 'nativeRuntime', backends: [] } : { type: 'done' }; } stop() { shutdown.stops++; return stopped; } }`,
	configurationMainService: `import { Event } from '${root}/src/base/common/event.js'; import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js'; import { state } from 'electron';
		export const IConfigurationMainService = createDecorator('configurationMainService');
		export class ConfigurationMainService { get snapshot() { return { restricted: state.background ? [] : ['application.runInBackground'] }; } getValue() { return 'system'; } onDidChangeConfiguration = Event.None; async initialize() { } async flush() { } async restoreEngineDefaults() { } }`,
	windowsMainService: `import { Event } from '${root}/src/base/common/event.js'; import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js'; import { EventEmitter } from 'node:events'; import { state } from 'electron';
		export const IWindowsMainService = createDecorator('windowsMainService');
		export class WindowsMainService { onDidOpenWindow = Event.None; window; focusMainWindow() { state.focused = (state.focused ?? 0) + 1; } getMainWindow() { return state.windowOpen ? this.window : undefined; } async openMainWindow() { state.windows++; state.windowOpen = true; this.window = Object.assign(new EventEmitter(), { webContents: new EventEmitter() }); return this.window; } }`,
	hostingMainService: `import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js'; import { state } from 'electron';
		export const IHostingMainService = createDecorator('hostingMainService');
		export class HostingMainService { start() { state.hosting = (state.hosting ?? 0) + 1; } }`,
	screenHostingMainService: `import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js'; import { state } from 'electron';
		export const IScreenHostingMainService = createDecorator('screenHostingMainService');
		export class ScreenHostingMainService { async start() { state.screenHosting = (state.screenHosting ?? 0) + 1; } }`,
	systemSettings: `import { state } from 'electron';
		export function setRunInBackground(value) { state.background = value; }
		export function launchedAtLogin() { return state.atLogin === true; }`,
	trayMainService: `import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js'; import { state } from 'electron';
		export const ITrayMainService = createDecorator('trayMainService');
		export class TrayMainService { onDidRequestWindow = listener => { state.requestWindow = listener; return { dispose() { } }; }; create() { state.trays = (state.trays ?? 0) + 1; } }`,
	traySupport: `import { state } from 'electron'; export async function traySupported() { return state.traySupported !== false; }`,
	nativeHostMainService: `import { Event } from '${root}/src/base/common/event.js'; import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js';
		export const INativeHostMainService = createDecorator('nativeHostMainService');
		export class NativeHostMainService { onDidChangeWindowState = Event.None; }`,
	permissionsMainService: `import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js';
		export const IPermissionsMainService = createDecorator('permissionsMainService');
		export class PermissionsMainService { async synchronize() { } }`,
	recordingMainService: `import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js';
		export const IRecordingMainService = createDecorator('recordingMainService');
		export class RecordingMainService { }`,
	encryptionMainService: `export class EncryptionMainService { }`,
	systemActionNotice: `import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js'; import { state } from 'electron';
		export const ISystemActionNoticeMainService = createDecorator('systemActionNoticeMainService');
		export class SystemActionNoticeMainService { start() { state.notices = (state.notices ?? 0) + 1; } }`,
	stateService: `import { state } from 'electron';
		export const SaveStrategy = { IMMEDIATE: 0, DELAYED: 1 };
		export class StateService { async init() { state.stateLoaded = true; } getItem() { } setItem() { } setItems() { } removeItem() { } async close() { state.stateClosed = (state.stateClosed ?? 0) + 1; } dispose() { } }`,
	'ipc.electron': `export class Server { registerChannel() { } dispose() { } }`,
	ipcMain: `export const WORKBENCH_URL = 'vnilink://app/index.html'; export const validatedIpcMain = { on() { }, handle() { }, removeHandler() { } };`,
	development: `export function registerDevelopmentWindow() { }`,
	menubar: `export function installApplicationMenu() { } export function registerEditingShortcuts() { }`,
	// The log file's native addon: the test writes no log.
	spdlogAddon: `export const SPDLOG_DIRECTORY = 'native/spdlog';
		const logger = { trace() { }, debug() { }, info() { }, warn() { }, error() { }, critical() { }, getLevel: () => 0, setLevel() { }, setPattern() { }, clearFormatters() { }, flush() { }, drop() { } };
		export function loadSpdlog() { return { setFlushOn() { }, createAsyncRotatingLogger: async () => logger }; }`,
};

async function load(entry, exports, extra = '') {
	const output = await build({
		stdin: { contents: `export { ${exports} } from '${entry}'; export { app, state } from 'electron'; ${extra}`, loader: 'ts', resolveDir: root }, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json', plugins: [{
			name: 'main-process-fixtures', setup(builder) {
				builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
				builder.onResolve({ filter: /\/(engineMainService|configurationMainService|windowsMainService|nativeHostMainService|permissionsMainService|recordingMainService|encryptionMainService|ipc\.electron|ipcMain|development|menubar|spdlogAddon|hostingMainService|screenHostingMainService|systemSettings|trayMainService|traySupport|systemActionNotice|stateService)\.js$/ }, ({ path }) => entry.endsWith('lifecycleMainService.ts') ? undefined : ({ path: path.split('/').at(-1).replace(/\.js$/, ''), namespace: 'fixture' }));
				builder.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: path === 'electron' ? electron : services[path], loader: 'js', resolveDir: root }));
			}
		}]
	});
	return await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
}

test('repeated quit requests wait for every shutdown participant once', async () => {
	const { LifecycleMainService, app, state } = await load('./src/platform/lifecycle/electron-main/lifecycleMainService.ts', 'LifecycleMainService');
	const lifecycle = new LifecycleMainService({ error() { } });
	let joins = 0;
	let release;
	lifecycle.onWillShutdown(event => {
		joins++;
		event.join('engine', new Promise(resolve => {
			release = resolve;
		}));
	});
	app.quit();
	app.quit();
	assert.equal(state.completed, false);
	assert.equal(joins, 1);
	release();
	await new Promise(resolve => setImmediate(resolve));
	assert.equal(state.completed, true);
	assert.equal(state.attempts, 3);
	assert.equal(joins, 1);
});

test('relaunch restarts a packaged application once and quits through shutdown', async () => {
	const { LifecycleMainService, state } = await load('./src/platform/lifecycle/electron-main/lifecycleMainService.ts', 'LifecycleMainService');
	const lifecycle = new LifecycleMainService({ error() { } });
	lifecycle.relaunch();
	lifecycle.relaunch();
	await new Promise(resolve => setImmediate(resolve));
	assert.equal(state.relaunched, 1);
	assert.equal(state.completed, true);
});

test('activation during startup does not open a second window and quitting waits for the engine', async () => {
	const { VniLinkApplication, app, state, finish, shutdown } = await load('./src/code/electron-main/app.ts', 'VniLinkApplication', `export { finish, shutdown } from './src/platform/engine/electron-main/engineMainService.js';`);
	const application = new VniLinkApplication({
		temporaryEngine: true, showWindow: false, onReady: async () => {
			app.quit();
			app.quit();
		}
	});
	await application.start();
	assert.equal(state.windows, 1, 'Activation during startup created a second window');
	assert.equal(state.stateLoaded, true, 'The window opened before UI state was read');
	assert.equal(state.completed, false, 'Quit completed before the engine stopped');
	assert.equal(shutdown.stops, 1);
	finish();
	await new Promise(resolve => setImmediate(resolve));
	assert.equal(state.completed, true);
	assert.equal(shutdown.stops, 1);
	assert.equal(state.stateClosed, 1, 'Quitting did not save UI state');
});

test('relaunching or activating without a window opens exactly one window', async () => {
	const { VniLinkApplication, app, state } = await load('./src/code/electron-main/app.ts', 'VniLinkApplication');
	const application = new VniLinkApplication({ temporaryEngine: true, showWindow: false });
	await application.start();
	assert.equal(state.windows, 1);
	app.emit('second-instance');
	assert.equal(state.focused, 1, 'An open window is focused, not replaced');
	state.windowOpen = false;
	app.emit('second-instance');
	app.emit('activate');
	await new Promise(resolve => setImmediate(resolve));
	assert.equal(state.windows, 2, 'Relaunch and activation together opened more than one window');
	assert.equal(state.trays, 1, 'The tray was not created where it is supported');
	state.windowOpen = false;
	state.requestWindow();
	await new Promise(resolve => setImmediate(resolve));
	assert.equal(state.windows, 3, "The tray's Open did not reopen the window");
	assert.equal(state.screenHosting, undefined, 'A temporary engine that may not capture shared the screen');
	application.dispose();
});

test('a temporary engine that may capture shares the screen', async () => {
	const { VniLinkApplication, state } = await load('./src/code/electron-main/app.ts', 'VniLinkApplication');
	const application = new VniLinkApplication({ temporaryEngine: true, temporaryEngineCaptures: true, showWindow: false });
	await application.start();
	assert.equal(state.screenHosting, 1);
	application.dispose();
});

test('closing the last window quits unless the application runs in the background', async () => {
	for (const background of [false, true]) {
		const { VniLinkApplication, app, state, setRunInBackground, finish } = await load('./src/code/electron-main/app.ts', 'VniLinkApplication', `export { setRunInBackground } from './src/platform/configuration/electron-main/systemSettings.js'; export { finish } from './src/platform/engine/electron-main/engineMainService.js'; export const variant = ${String(background)};`);
		setRunInBackground(background);
		const application = new VniLinkApplication({ temporaryEngine: true, showWindow: false });
		await application.start();
		state.windowOpen = false;
		app.emit('window-all-closed');
		assert.equal(state.attempts, background ? 0 : 1);
		finish();
		application.dispose();
	}
});

test('a login launch that runs in the background hosts without opening a window', async () => {
	const { VniLinkApplication, app, state, setRunInBackground } = await load('./src/code/electron-main/app.ts', 'VniLinkApplication', `export { setRunInBackground } from './src/platform/configuration/electron-main/systemSettings.js';`);
	setRunInBackground(true);
	state.atLogin = true;
	const application = new VniLinkApplication({});
	await application.start();
	assert.equal(state.windows, 0);
	assert.equal(state.hosting, 1);
	assert.equal(state.screenHosting, 1, 'The installed application did not share the screen');
	app.emit('activate');
	await new Promise(resolve => setImmediate(resolve));
	assert.equal(state.windows, 1, 'Activation after a hidden start did not open the window');
	application.dispose();
});

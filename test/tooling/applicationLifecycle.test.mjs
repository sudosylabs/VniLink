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
export class MessageChannelMain { }`;

/** Main services the composition test replaces; each records only what the test asserts. */
const services = {
	engineMainService: `import { Event } from '${root}/src/base/common/event.js'; import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js'; import { app } from 'electron';
		export const IEngineMainService = createDecorator('engineMainService');
		export const shutdown = { stops: 0 }; let release; export const stopped = new Promise(resolve => { release = resolve; }); export function finish() { release(); }
		export class EngineMainService { available = true; onDidChangeAvailability = Event.None; onDidChangeRestartRequired = Event.None; async start() { app.emit('activate'); } async refreshRestartRequired() { } async execute(command) { return command.type === 'nativeRuntime' ? { type: 'nativeRuntime', backends: [] } : { type: 'done' }; } stop() { shutdown.stops++; return stopped; } }`,
	configurationMainService: `import { Event } from '${root}/src/base/common/event.js'; import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js';
		export const IConfigurationMainService = createDecorator('configurationMainService');
		export class ConfigurationMainService { getValue() { return 'system'; } onDidChangeConfiguration = Event.None; async initialize() { } async flush() { } async restoreEngineDefaults() { } }`,
	windowsMainService: `import { Event } from '${root}/src/base/common/event.js'; import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js'; import { EventEmitter } from 'node:events'; import { state } from 'electron';
		export const IWindowsMainService = createDecorator('windowsMainService');
		export class WindowsMainService { onDidOpenWindow = Event.None; focusMainWindow() { } async openMainWindow() { state.windows++; return Object.assign(new EventEmitter(), { webContents: new EventEmitter() }); } }`,
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
	'ipc.electron': `export class Server { registerChannel() { } dispose() { } }`,
	ipcMain: `export const WORKBENCH_URL = 'vnilink://app/index.html'; export const validatedIpcMain = { handle() { }, removeHandler() { } };`,
	development: `export function registerDevelopmentWindow() { }`,
};

async function load(entry, exports, extra = '') {
	const output = await build({
		stdin: { contents: `export { ${exports} } from '${entry}'; export { app, state } from 'electron'; ${extra}`, loader: 'ts', resolveDir: root }, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json', plugins: [{
			name: 'main-process-fixtures', setup(builder) {
				builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
				builder.onResolve({ filter: /\/(engineMainService|configurationMainService|windowsMainService|nativeHostMainService|permissionsMainService|recordingMainService|encryptionMainService|ipc\.electron|ipcMain|development)\.js$/ }, ({ path }) => entry.endsWith('lifecycleMainService.ts') ? undefined : ({ path: path.split('/').at(-1).replace(/\.js$/, ''), namespace: 'fixture' }));
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
	assert.equal(state.completed, false, 'Quit completed before the engine stopped');
	assert.equal(shutdown.stops, 1);
	finish();
	await new Promise(resolve => setImmediate(resolve));
	assert.equal(state.completed, true);
	assert.equal(shutdown.stops, 1);
});

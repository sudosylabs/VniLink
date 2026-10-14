import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const root = process.cwd().replaceAll('\\', '/');

const fixtures = {
	state: `export const state = { stops: 0, attempts: 0, completed: false, windows: 0 }; let release; export const stopped = new Promise(resolve => { release = resolve; }); export function finish() { release(); }`,
	electron: `import { EventEmitter } from 'node:events'; import { state } from 'fixture:state'; export const app = new EventEmitter(); Object.assign(app, { isPackaged:false, getPath:()=>'/fixture', setName(){}, requestSingleInstanceLock:()=>true, whenReady:()=>Promise.resolve(), quit(){ state.attempts++; let prevented=false; this.emit('before-quit',{preventDefault(){prevented=true;}}); if(!prevented){state.completed=true;this.emit('will-quit');} }, exit(){throw new Error('Unexpected fatal exit');} }); export const protocol={registerSchemesAsPrivileged(){},handle(){}}; export const session={defaultSession:{setPermissionRequestHandler(){},setPermissionCheckHandler(){}}}; export const nativeTheme={}; export const systemPreferences={}; export const net={}; export const BrowserWindow={getAllWindows:()=>[]};`,
	engine: `import { state, stopped } from 'fixture:state'; import {app} from 'electron'; export class EngineService { available=true; async start(){app.emit('activate');} async execute(command){return command.type==='nativeRuntime'?{type:'nativeRuntime',backends:[]}:{type:'done'};} stop(){state.stops++;return stopped;} }`,
	configuration: `import { Event } from '${root}/src/base/common/event.js'; import { defaultConfiguration } from '${root}/src/platform/configuration/common/configuration.js'; export class ConfigurationStore { snapshot={values:defaultConfiguration}; onDidChangeConfiguration=Event.None; async initialize(){} async flush(){} dispose(){} }`,
	native: `import { Event } from '${root}/src/base/common/event.js'; import { createDecorator } from '${root}/src/platform/instantiation/common/instantiation.js'; export const INativeHostMainService=createDecorator('nativeHost'); export class NativeHostMainService { constructor(getWindow){this.getWindow=getWindow;} onDidChangeWindowState=Event.None; setNativeBackgroundEnabled(){} registerWindow(){} }`,
	window: `import { EventEmitter } from 'node:events'; import {state} from 'fixture:state'; export function createMainWindow(){state.windows++;return Object.assign(new EventEmitter(),{webContents:{mainFrame:{url:'vnilink://app/index.html'}},async loadURL(){},show(){},isDestroyed:()=>false,destroy(){}});}`,
	ipc: `export function registerDesktopHandlers(){return {dispose(){}};}`,
	encryption: `export class EncryptionMainService {}`,
};

const names = { 'engineService.js': 'engine', 'configurationStore.js': 'configuration', 'nativeHostMainService.js': 'native', 'windows.js': 'window', 'handlers.js': 'ipc', 'encryptionMainService.js': 'encryption' };

const output = await build({
	stdin: { contents: `export {VniLinkApplication} from './src/app/electron-main/application.js'; export {app} from 'electron'; export {state,finish} from 'fixture:state';`, loader: 'ts', resolveDir: root }, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json', plugins: [{
		name: 'application-lifetime', setup(builder) {
			builder.onResolve({ filter: /^electron$|^fixture:/ }, ({ path }) => ({ path: path === 'electron' ? path : path.slice(8), namespace: 'fixture' }));
			builder.onResolve({ filter: /\/(engineService|configurationStore|nativeHostMainService|windows|handlers|encryptionMainService)\.js$/ }, ({ path }) => ({ path: names[path.split('/').at(-1)], namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: fixtures[path], loader: 'js', resolveDir: root }));
		}
	}]
});

const { VniLinkApplication, app, state, finish } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

test('repeated quit requests cannot bypass pending engine shutdown', async () => {
	const application = new VniLinkApplication({
		temporaryEngine: true, showWindow: false, onReady: async () => {
			app.quit();
			app.quit();
		}
	});
	await application.start();
	assert.equal(state.windows, 1, 'Activation during startup created a second window');
	assert.equal(state.completed, false);
	assert.equal(state.stops, 1);
	finish();
	await new Promise(resolve => setImmediate(resolve));
	assert.equal(state.completed, true);
	assert.equal(state.attempts, 3);
	assert.equal(state.stops, 1);
});

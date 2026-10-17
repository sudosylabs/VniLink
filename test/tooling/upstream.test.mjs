import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';
import { isApplicationFile } from '../../build/sourceScope.mjs';
import staged from '../../.lintstagedrc.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));

test('staged fixes preserve imported source while checking application files', () => {
	const imported = `${root}src/platform/instantiation/common/instantiationService.ts`;
	const application = `${root}src/workbench/parts/titlebar/titlebarPart.ts`;
	assert.equal(isApplicationFile(imported), false);
	assert.equal(isApplicationFile(application), true);
	const commands = staged['{src,test}/**/*.ts']([imported, application]);
	assert.equal(commands.length, 2);
	for (const command of commands) {
		assert.ok(command.includes(JSON.stringify(application)));
		assert.ok(!command.includes(JSON.stringify(imported)));
	}
	assert.deepEqual(staged['{src,test}/**/*.ts']([imported]), []);
});

test('copied service container injects, shares and disposes its service instances', async () => {
	const bundle = await build({
		stdin: {
			contents: `import { createDecorator } from './src/platform/instantiation/common/instantiation.js';
interface ProbeService { readonly _serviceBrand: undefined; }
export const ProbeId = createDecorator<ProbeService>('vnilink-upstream-probe');
export class Consumer { constructor(@ProbeId public readonly service: ProbeService) {} }
export { InstantiationService } from './src/platform/instantiation/common/instantiationService.js';
export { ServiceCollection } from './src/platform/instantiation/common/serviceCollection.js';
export { SyncDescriptor } from './src/platform/instantiation/common/descriptors.js';`,
			resolveDir: root, loader: 'ts',
		},
		bundle: true, write: false, format: 'esm', platform: 'node', target: 'node22',
		tsconfig: `${root}src/tsconfig.json`,
	});
	const { ProbeId: id, Consumer, InstantiationService, ServiceCollection, SyncDescriptor } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
	let created = 0;
	let disposed = 0;
	class Service {
		constructor() {
			created++;
		}

		dispose() {
			disposed++;
		}
	}
	const container = new InstantiationService(new ServiceCollection([id, new SyncDescriptor(Service)]), true);
	assert.equal(created, 0);
	const consumer = container.createInstance(Consumer);
	assert.equal(created, 1);
	assert.equal(container.invokeFunction(accessor => accessor.get(id)), consumer.service);
	const child = container.createChild(new ServiceCollection());
	assert.equal(child.createInstance(Consumer).service, consumer.service);
	child.dispose();
	assert.equal(disposed, 0);
	container.dispose();
	assert.equal(disposed, 1);
	assert.throws(() => container.createInstance(Consumer), /disposed/);
});

test('native-host proxy sends its window as context, forwards window events and releases them', async () => {
	const bundle = await build({
		stdin: {
			contents: `export { NativeHostService } from './src/platform/native/common/nativeHostService.js';
 export { IMainProcessService } from './src/platform/ipc/common/mainProcessService.js';
 export { INativeHostService, isNativeHostRequest } from './src/platform/native/common/native.js';
 export { InstantiationService } from './src/platform/instantiation/common/instantiationService.js';
 export { ServiceCollection } from './src/platform/instantiation/common/serviceCollection.js';
 export { SyncDescriptor } from './src/platform/instantiation/common/descriptors.js';`, resolveDir: root, loader: 'ts'
		},
		bundle: true, write: false, format: 'esm', platform: 'node', target: 'node22', tsconfig: `${root}src/tsconfig.json`,
	});
	const { NativeHostService, IMainProcessService, INativeHostService, InstantiationService, ServiceCollection, SyncDescriptor, isNativeHostRequest } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
	let receive;
	let unsubscribed = 0;
	const calls = [];
	const state = { windowId: 7, revision: 1, focused: false, maximized: false, fullscreen: false, alwaysOnTop: false, zoomFactor: 1, backgroundMaterial: 'none' };
	const channel = {
		async call(method, args) {
			calls.push([method, args]);
			return method === 'getWindowState' ? state : undefined;
		},
		listen(event) {
			assert.equal(event, 'onDidChangeWindowState');
			return listener => {
				receive = listener;
				return { dispose: () => unsubscribed++ };
			};
		},
	};
	const mainProcess = { getChannel: name => name === 'nativeHost' ? channel : assert.fail(`Unexpected channel ${name}`) };
	const container = new InstantiationService(new ServiceCollection([IMainProcessService, mainProcess], [INativeHostService, new SyncDescriptor(NativeHostService, [7])]), true);
	const host = container.invokeFunction(accessor => accessor.get(INativeHostService));
	const events = [];
	const subscription = host.onDidChangeWindowState(value => events.push(value));
	await host.focusWindow();
	assert.deepEqual(await host.getWindowState(), state);
	await host.pickFolder('/Pictures');
	assert.deepEqual(calls, [['focusWindow', [7]], ['getWindowState', [7]], ['pickFolder', [7, '/Pictures']]]);
	receive({ ...state, revision: 3, focused: true });
	assert.deepEqual(events, [{ ...state, revision: 3, focused: true }]);
	assert.equal(host.windowId, 7);
	assert.equal(isNativeHostRequest('setFullScreen', [7, 'yes']), false);
	assert.equal(isNativeHostRequest('updateWindowControls', [7, { height: Infinity }]), false);
	assert.equal(isNativeHostRequest('focusWindow', [7, { targetWindowId: 8 }]), false);
	assert.equal(isNativeHostRequest('openExternal', [7, '']), false);
	assert.equal(isNativeHostRequest('relaunch', [7, true]), false);
	assert.equal(isNativeHostRequest('execute', [7]), false);
	subscription.dispose();
	container.dispose();
	assert.equal(unsubscribed, 1);
	assert.equal(calls.some(([method]) => method === 'dispose'), false);
});

import { Event } from '../../src/base/common/event.js';
import { IConfigurationService, defaultConfiguration } from '../../src/platform/configuration/common/configuration.js';
import { SyncDescriptor } from '../../src/platform/instantiation/common/descriptors.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { Workbench } from '../../src/workbench/browser/workbench.js';

export async function verifyWorkbenchLifecycle(): Promise<void> {
	let resolve: (() => void) | undefined;
	let disposed = 0;
	let created = 0;
	class Configuration {
		declare public readonly _serviceBrand: undefined;
		public readonly snapshot = { revision: 0, values: defaultConfiguration, error: null };
		public readonly onDidChangeConfiguration = Event.None;

		public initialize(): Promise<void> {
			return new Promise<void>(finish => {
				resolve = finish;
			});
		}

		public updateValue(): Promise<void> {
			return Promise.resolve();
		}

		public dispose(): void {
			disposed++;
		}
	}
	const parent = document.createElement('div');
	const workbench = new Workbench(parent, { windowId: 1, osRelease: 'test', platform: 'darwin', version: 'test', engineAvailable: false, enableSmokeTestDriver: false }, new ServiceCollection([IConfigurationService, new SyncDescriptor(Configuration)]), () => {
		created++;
		return {
			create() {
				throw new Error('Disposed startup rendered');
			}, initialize: () => Promise.resolve(), dispose() { /* No native resources in this fixture. */ }
		};
	});
	const start = workbench.startup();
	workbench.dispose();
	resolve?.();
	let rejected = false;
	try {
		await start;
	} catch {
		rejected = true;
	}
	if (!rejected || created !== 0 || disposed !== 1 || parent.children.length !== 0) {
		throw new Error('Disposed startup created parts or retained services');
	}
	if (workbench.startup() !== start) {
		throw new Error('Startup created multiple lifetimes');
	}
}

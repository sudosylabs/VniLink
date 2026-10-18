import { Event } from '../../src/base/common/event.js';
import { IWorkbenchConfigurationService } from '../../src/workbench/services/configuration/configurationService.js';
import { SyncDescriptor } from '../../src/platform/instantiation/common/descriptors.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { NullLogService } from '../../src/platform/log/common/log.js';
import { Workbench } from '../../src/workbench/workbench.js';

export async function verifyWorkbenchLifecycle(): Promise<void> {
	let resolve: (() => void) | undefined;
	let disposed = 0;
	class Configuration {
		declare public readonly _serviceBrand: undefined;
		public readonly onDidChangeConfiguration = Event.None;

		public initialize(): Promise<void> {
			return new Promise<void>(finish => {
				resolve = finish;
			});
		}

		public dispose(): void {
			disposed++;
		}
	}
	const parent = document.createElement('div');
	const workbench = new Workbench(parent, { windowId: 1, osRelease: 'test', platform: 'darwin', version: 'test', engineAvailable: false, enableSmokeTestDriver: false }, new ServiceCollection([IWorkbenchConfigurationService, new SyncDescriptor(Configuration)]), new NullLogService());
	const start = workbench.startup();
	workbench.dispose();
	resolve?.();
	let rejected = false;
	try {
		await start;
	} catch {
		rejected = true;
	}
	if (!rejected || disposed !== 1 || parent.children.length !== 0) {
		throw new Error('Disposed startup rendered parts or retained services');
	}
	if (workbench.startup() !== start) {
		throw new Error('Startup created multiple lifetimes');
	}
}

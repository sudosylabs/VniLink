import './media/workbench.css';
import { runWhenWindowIdle } from '../base/browser/dom.js';
import { ensureCodeWindow, mainWindow } from '../base/browser/window.js';
import { errorHandler, isCancellationError, setUnexpectedErrorHandler } from '../base/common/errors.js';
import { IConfigurationSnapshotService } from '../platform/configuration/common/configurationSnapshot.js';
import { IEngineService } from '../platform/engine/common/engineService.js';
import { getSingletonServiceDescriptors } from '../platform/instantiation/common/extensions.js';
import { IInstantiationService, type ServicesAccessor } from '../platform/instantiation/common/instantiation.js';
import { InstantiationService } from '../platform/instantiation/common/instantiationService.js';
import type { ServiceCollection } from '../platform/instantiation/common/serviceCollection.js';
import type { ILogService } from '../platform/log/common/log.js';
import type { INativeWindowConfiguration } from '../platform/window/common/windowConfiguration.js';
import { WorkbenchContributionsRegistry } from './contributions.js';
import { Layout } from './layout.js';
import { NativeTitlebarPart } from './parts/titlebar/nativeTitlebarPart.js';
import { ViewsPart } from './parts/views/viewsPart.js';
import { registerWindowDriver } from './services/driver/driver.js';
import { ILifecycleService, LifecyclePhase } from './services/lifecycle/lifecycle.js';

/**
 * The window's workbench, in the shape of VS Code's `Workbench`: it creates
 * the instantiation service, starts contributions, renders the parts and
 * moves the lifecycle through Ready, Restored and Eventually.
 */
export class Workbench extends Layout {
	private startupPromise: Promise<void> | undefined;

	constructor(
		parent: HTMLElement,
		private readonly configuration: INativeWindowConfiguration,
		private readonly serviceCollection: ServiceCollection,
		private readonly logService: ILogService,
	) {
		super(parent);
		this.registerErrorHandler();
	}

	/** Starts the workbench once; disposing it during startup rejects. */
	public startup(): Promise<void> {
		this.startupPromise ??= this.start().catch((error: unknown) => {
			this.dispose();
			throw error;
		});
		return this.startupPromise;
	}

	private registerErrorHandler(): void {
		const previous = errorHandler.getUnexpectedErrorHandler();
		this._register({
			dispose: () => {
				setUnexpectedErrorHandler(previous);
			},
		});
		setUnexpectedErrorHandler(error => {
			if (isCancellationError(error) || this._store.isDisposed) {
				return;
			}
			this.logService.error(error instanceof Error ? error : new Error(String(error)));
			this.setNotice(Layout.unexpectedErrorNotice);
		});
	}

	private async start(): Promise<void> {
		ensureCodeWindow(mainWindow, this.configuration.windowId);
		const instantiationService = this.initServices();
		await instantiationService.invokeFunction(accessor => accessor.get(IConfigurationSnapshotService).initialize());
		this.checkAlive();
		const titlebar = instantiationService.invokeFunction(accessor => {
			this.registerListeners(accessor);
			WorkbenchContributionsRegistry.INSTANCE.start(accessor);
			return this.renderWorkbench(accessor);
		});
		await titlebar.restore();
		this.checkAlive();
		this.restore(instantiationService);
	}

	private initServices(): InstantiationService {
		for (const [id, descriptor] of getSingletonServiceDescriptors()) {
			this.serviceCollection.set(id, descriptor);
		}
		return this._register(new InstantiationService(this.serviceCollection, true));
	}

	private registerListeners(accessor: ServicesAccessor): void {
		const engineService = accessor.get(IEngineService);
		const lifecycleService = accessor.get(ILifecycleService);
		const updateEngineNotice = (): void => {
			this.setNotice(engineService.available ? undefined : Layout.engineUnavailableNotice);
		};
		this._register(engineService.onDidChangeAvailability(updateEngineNotice));
		updateEngineNotice();
		this._register({
			dispose: () => {
				void lifecycleService.shutdown();
				delete document.documentElement.dataset.workbenchReady;
			},
		});
	}

	private renderWorkbench(accessor: ServicesAccessor): NativeTitlebarPart {
		const instantiationService = accessor.get(IInstantiationService);
		document.documentElement.dataset.platform = this.configuration.platform;
		const titlebar = this._register(instantiationService.createInstance(NativeTitlebarPart, this.configuration.osRelease));
		const views = this._register(instantiationService.createInstance(ViewsPart));
		this.createWorkbenchLayout(titlebar, views);
		accessor.get(ILifecycleService).phase = LifecyclePhase.Ready;
		return titlebar;
	}

	private restore(instantiationService: InstantiationService): void {
		instantiationService.invokeFunction(accessor => {
			const lifecycleService = accessor.get(ILifecycleService);
			lifecycleService.phase = LifecyclePhase.Restored;
			if (this.configuration.enableSmokeTestDriver) {
				registerWindowDriver(instantiationService);
			}
			document.documentElement.dataset.workbenchReady = 'true';
			this._register(runWhenWindowIdle(mainWindow, () => {
				lifecycleService.phase = LifecyclePhase.Eventually;
			}, 2500));
		});
	}

	private checkAlive(): void {
		if (this._store.isDisposed) {
			throw new Error('Workbench disposed during startup');
		}
	}
}

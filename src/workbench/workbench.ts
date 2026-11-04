// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import './media/workbench.css';
import { runWhenWindowIdle } from '../base/browser/dom.js';
import { ensureCodeWindow, mainWindow } from '../base/browser/window.js';
import { errorHandler, isCancellationError, setUnexpectedErrorHandler } from '../base/common/errors.js';
import { IWorkbenchConfigurationService } from './services/configuration/configurationService.js';
import * as nls from '../nls.js';
import { IEngineService } from '../platform/engine/common/engineService.js';
import type { ILogService } from '../platform/log/common/log.js';
import { IDialogService } from './services/dialogs/dialogService.js';
import { getSingletonServiceDescriptors } from '../platform/instantiation/common/extensions.js';
import { ILayoutService } from '../platform/layout/browser/layoutService.js';
import { IInstantiationService, type ServicesAccessor } from '../platform/instantiation/common/instantiation.js';
import { InstantiationService } from '../platform/instantiation/common/instantiationService.js';
import type { ServiceCollection } from '../platform/instantiation/common/serviceCollection.js';
import type { INativeWindowConfiguration } from '../platform/window/common/windowConfiguration.js';
import { WorkbenchContributionsRegistry } from './contributions.js';
import { Layout } from './layout.js';
import { NativeTitlebarPart } from './parts/titlebar/nativeTitlebarPart.js';
import { ViewsPart } from './parts/views/viewsPart.js';
import { registerWindowDriver } from './services/driver/driver.js';
import { IHostService } from './services/host/host.js';
import { ILifecycleService, LifecyclePhase } from './services/lifecycle/lifecycle.js';

/**
 * The window's workbench: it creates the instantiation service, starts contributions, renders the parts and
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
		await instantiationService.invokeFunction(accessor => accessor.get(IWorkbenchConfigurationService).initialize());
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
		this.serviceCollection.set(ILayoutService, this);
		for (const [id, descriptor] of getSingletonServiceDescriptors()) {
			this.serviceCollection.set(id, descriptor);
		}
		return this._register(new InstantiationService(this.serviceCollection, true));
	}

	private registerListeners(accessor: ServicesAccessor): void {
		const engineService = accessor.get(IEngineService);
		const lifecycleService = accessor.get(ILifecycleService);
		const dialogService = accessor.get(IDialogService);
		const hostService = accessor.get(IHostService);
		this._register(hostService.onDidChangeFocus(hasFocus => {
			this.onWindowFocusChanged(hasFocus);
		}));
		this.onWindowFocusChanged(hostService.hasFocus);
		const startFresh = async (): Promise<void> => {
			const { button } = await dialogService.show({
				type: 'warning',
				message: nls.localize('workbench.startFreshTitle', 'Start VniLink fresh on this computer?'),
				detail: nls.localize('workbench.startFreshDetail', 'VniLink can’t read the installation it saved here, as can happen after updating from an older test version. Starting fresh gives this computer a new identity: pair your other computers with it again. What it shares starts from the defaults. The unreadable installation is kept aside, not deleted.'),
				buttons: [nls.localize('workbench.startFresh', 'Start Fresh'), nls.localize('workbench.startFreshCancel', 'Cancel')],
				cancelId: 1,
			});
			if (button === 0) {
				// The notice follows the engine: back once it runs, or this one again if it still cannot read.
				await engineService.startFresh().catch((error: unknown) => {
					this.logService.error('Starting fresh failed.', error);
				});
			}
		};
		const updateEngineNotice = (): void => {
			if (engineService.available) {
				this.setNotice(undefined);
			} else if (engineService.installationUnreadable) {
				this.setNotice(Layout.installationUnreadableNotice, {
					label: nls.localize('workbench.startFreshAction', 'Start Fresh…'),
					run: () => void startFresh(),
				});
			} else {
				this.setNotice(engineService.restarting ? Layout.engineRestartingNotice : Layout.engineUnavailableNotice);
			}
		};
		this._register(engineService.onDidChangeAvailability(updateEngineNotice));
		this._register(engineService.onDidChangeRestarting(updateEngineNotice));
		this._register(engineService.onDidChangeInstallationUnreadable(updateEngineNotice));
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
